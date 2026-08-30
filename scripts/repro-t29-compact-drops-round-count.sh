#!/bin/sh
# T29 复现：`zloop compact`（整理账本）把老 todo 的 tick 搬进 `archive/`，而「这个目标跑了
# 几轮」是**从 ticks 现数出来的**。于是一次例行整理之后：
#
#   * `zloop status` 印「跑了 0 轮」——刚才还是 4 轮；
#   * `zloop stats` 更狠：它在 rounds==0 时印一句「还没有跑过任何一轮 · zloop next 开始」
#     然后**直接返回**，一个跑了 4 轮、完成过 2 条 todo 的目标被说成从没开工；
#   * `zloop replan` 的 rework 信号（rounds>=3 且返工率>=0.5）跟着熄火——
#     返工率 50% 的目标，整理一次就不再触发重估了。
#
# 和 A-18 是同一个根因的两半：A-18 是**花费**被搬走（预算闸静默复位），这条是**轮次**
# 被搬走。凡是「从 ticks 现算的累计量」，compact 都会一起带走，所以修法不是再加一个
# 计数器，而是让归档汇总按 outcome 记下来，`status` / `stats` 两处一起从它补回来。
#
#   sh scripts/repro-t29-compact-drops-round-count.sh
#
# 退出码 1 = 复现成功（整理之后轮次掉了）；0 = 修好了。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

W=$(mktemp -d "/tmp/zloop-t29.XXXXXX") || exit 2
cd "$W" || exit 2
"$ZLOOP" init "t29 repro" >/dev/null
"$ZLOOP" plan --add "[P0] 老活一" --add "[P0] 老活二" --add "[P1] 还没做的活" >/dev/null

# 4 轮：done / progress / fail / done —— 返工 2 轮，返工率正好压在 replan 的 0.5 上
"$ZLOOP" done t1 --note "ok"        --no-doc >/dev/null
"$ZLOOP" done t2 --note "只做了一半" --outcome progress --no-doc >/dev/null
"$ZLOOP" done t2 --note "挂了"      --outcome fail     --no-doc >/dev/null
"$ZLOOP" done t2 --note "ok"        --no-doc >/dev/null

# 让前两条 todo 和它们的 tick 都变成一个月前的（compact 才会认它们是老账）
python3 - <<'PY'
import json, datetime
p = ".zloop/state.json"
s = json.load(open(p))
old = (datetime.datetime.now().astimezone() - datetime.timedelta(days=30)).isoformat()
for t in s["todos"]:
    if t["id"] != "t3":
        t["done_at"] = old
        t["updated_at"] = old
for k in s["ticks"]:
    k["at"] = old
json.dump(s, open(p, "w"), ensure_ascii=False, indent=2)
PY

show() {
  "$ZLOOP" status 2>/dev/null | grep -F '跑了' | sed 's/^/  status  /'
  # 第三处读数：轮次**编号**。它不是余额是编号，掉回去意味着同一个号发两次。
  "$ZLOOP" next --peek --json 2>/dev/null |
    python3 -c 'import json,sys; print("  next    round = %d" % json.load(sys.stdin)["round"])'
  "$ZLOOP" stats  2>/dev/null | grep -E '轮次|还没有跑过|归档' | sed 's/^ */  stats   /'
  # 管道里 grep 的退出码被 sed 盖掉，所以先落到变量再判：信号没了要**印出来**，
  # 不能只是少一行——少一行看着像是没输出，不像是闸没了。
  sig=$("$ZLOOP" replan 2>/dev/null | grep -F '返工率')
  echo "  replan  ${sig:-（没有返工信号：重估不会被触发了）}" | sed 's/- \[/[/'
}

echo "=== 整理之前 ==="
show

echo
echo "=== 人做了一次例行整理：zloop compact --keep-days 30 ==="
"$ZLOOP" compact --keep-days 30 | head -1 | sed 's/^/  /'

echo
echo "=== 整理之后 ==="
show
AFTER=$("$ZLOOP" stats --json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print("%d %d" % (d["rounds"], d["rework"]))')

echo
case "${AFTER}" in
  "4 2")
    echo "[OK] 整理不再抹掉轮次：跑了 4 轮 / 返工 2 都还在"
    exit 0;;
  *)
    echo "[FAIL] 复现成功：整理之前 4 轮 / 返工 2，整理之后 ${AFTER}"
    echo "       「这个目标跑了多久」被一次例行整理清零。工作目录：${W}"
    exit 1;;
esac
