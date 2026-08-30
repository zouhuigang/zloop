#!/bin/sh
# A-16 复现：人在终端里敲三下 `zloop next`，就能把一个本该「睡到配额窗口放开再接着跑」
# 的 runner 变成「拒绝启动 / 醒来直接退出」。
#
# 起因是 t22 那个问题：`max_noop_streak` 在 runner 路径上是不是死策略？
# 查下来比「死」更糟——它不是不生效，而是**跨进程串味**：
#   * `noop` tick 只有 `zloop next` 在 `should_run=false` 时会记（cli.rs:763），runner 一条不记；
#   * 但两边读的是同一本 state.json，`decide()` 在 `noop_streak >= max_noop_streak` 时
#     会把 `throttled` 那一支的 `interval_min` 翻成 `None`；
#   * `wait_plan()` 原来把 `None` 一律当「停」。
# 于是交互式的一次「我看一眼现在什么情况」，把无头长跑掐了。
#
#   sh scripts/repro-a16-noop-poke-kills-throttled-runner.sh
#
# 退出码 1 = 复现成功（敲过 next 之后 runner 不肯起来）；0 = 修好了（两种情况行为一致）。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

# 造一个「跑过一轮就撞配额窗口」的项目：max_runs=1，清单里还留着一条没做的。
setup() {
  W=$(mktemp -d "/tmp/zloop-a16.XXXXXX") || exit 2
  mkdir -p "$W/bin"
  cat > "$W/bin/claude" <<'EOF'
#!/bin/sh
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "假宿主" >/dev/null 2>&1
echo '{"session_id":"a16","is_error":false,"result":"ok"}'
EOF
  chmod +x "$W/bin/claude"
  ( cd "$W" || exit 2
    "$ZLOOP" init "a16 repro" >/dev/null
    "$ZLOOP" plan --add "[P0] 第一件事" --add "[P1] 第二件事" >/dev/null
    python3 - <<'PY'
import json
s = json.load(open(".zloop/state.json"))
s["policy"]["max_runs"] = 1              # 跑一轮就满
s["policy"]["intervals_min"] = [1, 1, 2] # --fast 下按秒算
json.dump(s, open(".zloop/state.json", "w"), ensure_ascii=False, indent=2)
PY
    # 真跑一轮：宿主自己 `zloop done`，窗口就此填满
    "$ZLOOP" run --host claude --fast --no-replan --max-rounds 1 >round1.log 2>&1
  )
  echo "$W"
}

PATH_FOR() { echo "$1/bin:$(dirname "$ZLOOP"):$PATH"; }

echo "=== 场景 A：没人碰过它 ==="
WA=$(setup); cd "$WA" || exit 2
PATH=$(PATH_FOR "$WA"); export PATH
"$ZLOOP" run --host claude --fast --no-replan >runA.log 2>&1 &
PIDA=$!
i=0; while [ $i -lt 6 ]; do kill -0 $PIDA 2>/dev/null || break; i=$((i+1)); sleep 1; done
if kill -0 $PIDA 2>/dev/null; then
  A="睡着（还活着）"; kill -TERM $PIDA 2>/dev/null; sleep 1; kill -KILL $PIDA 2>/dev/null
else
  A="退出了：$(grep -o 'runner: stop (.*)' runA.log | tail -1)"
fi
NOOP_A=$(grep -o '"outcome": *"noop"' .zloop/state.json 2>/dev/null | wc -l | tr -d " ")
echo "  runner: $A ｜ 账本里 runner 记的 noop：$NOOP_A 条"
grep -o '"event":"sleep"[^}]*' .zloop/runner/journal.jsonl | tail -1 | sed 's/^/  journal: /'

echo
echo "=== 场景 B：人敲了 3 下 zloop next（就想看一眼现在什么情况） ==="
WB=$(setup); cd "$WB" || exit 2
PATH=$(PATH_FOR "$WB"); export PATH
for _ in 1 2 3; do "$ZLOOP" next | sed 's/^/  $ zloop next → /'; done
NOOP_B=$(grep -o '"outcome": *"noop"' .zloop/state.json 2>/dev/null | wc -l | tr -d " ")
echo "  账本里现在有 $NOOP_B 条 noop（全是 zloop next 记的）"
"$ZLOOP" run --host claude --fast --no-replan >runB.log 2>&1 &
PIDB=$!
i=0; while [ $i -lt 6 ]; do kill -0 $PIDB 2>/dev/null || break; i=$((i+1)); sleep 1; done
if kill -0 $PIDB 2>/dev/null; then
  B="睡着（还活着）"; kill -TERM $PIDB 2>/dev/null; sleep 1; kill -KILL $PIDB 2>/dev/null
else
  B="退出了：$(grep -o 'runner: stop (.*)' runB.log | tail -1)"
fi
echo "  runner: $B"
"$ZLOOP" start --host claude --fast 2>&1 | head -1 | sed 's/^/  $ zloop start → /'
"$ZLOOP" stop >/dev/null 2>&1

echo
case "$B" in
  *"stop (throttled)"*)
    echo "[FAIL] 复现成功：同一个状态，只因为人敲过 3 下 zloop next，runner 就从「${A}」变成了「${B}」"
    exit 1;;
  *)
    echo "[OK] 两种情况行为一致（A=${A} / B=${B}）：noop 计数不再左右 runner 的停机判断"
    exit 0;;
esac
