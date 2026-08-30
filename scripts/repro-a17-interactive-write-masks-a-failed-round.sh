#!/bin/sh
# A-17 复现：人在另一个终端敲一句 `zloop feedback`，就能让一轮**失败**的宿主被记成
# 「写回了」——于是 `fail_streak` 永远涨不上去，连续失败停机这道闸整个失效。
#
# 这是 A-16 的同一类：交互式命令写进账本的东西串进了 runner 的判断。
# 只不过串进去的路不是 `noop_streak`，是**结算那一步的判据**（runner.rs:1069-1091）：
#
#     for i in ticks_before..st.ticks.len() {
#         if t.outcome != "noop" { wrote = true; }      // ← 「有人加了条 tick」
#     }
#     if !wrote && ... { tick::record(st, "fail", ...) }  // ← 当成「宿主写回了」
#
# 注释写的是 "did the host write back?"，代码问的是「这段时间里账本长了没长」。
# `noop` 被排除掉了（那正是 A-16 顺手补上的），但 `feedback` / `edit` / `replan`
# 一样是交互式命令记的、一样不是宿主的写回——而 `zloop feedback` 恰恰是文档教人
# 「跟正在跑的循环说话」的那条路，撞上的概率不低。
#
#   sh scripts/repro-a17-interactive-write-masks-a-failed-round.sh
#
# 退出码 1 = 复现成功（人插一句话，连续失败就停不下来了）；0 = 修好了（两边都停）。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

# 一个**每轮都失败**的假宿主：不写回、非零退出、慢到人来得及插话。
# max_fail_streak=2 → 正常情况下第 2 轮之后 runner 应该 stop(fail_streak)。
setup() {
  W=$(mktemp -d "/tmp/zloop-a17.XXXXXX") || exit 2
  mkdir -p "$W/bin"
  cat > "$W/bin/claude" <<'EOF'
#!/bin/sh
sleep 2
echo "host blew up" >&2
exit 1
EOF
  chmod +x "$W/bin/claude"
  ( cd "$W" || exit 2
    "$ZLOOP" init "a17 repro" >/dev/null
    "$ZLOOP" plan --add "[P0] 干点活" >/dev/null
    python3 - <<'PY'
import json
s = json.load(open(".zloop/state.json"))
s["policy"]["max_fail_streak"] = 2        # 连着 2 轮失败就该停
s["policy"]["intervals_min"] = [1, 1, 2]  # --fast 下按秒算
json.dump(s, open(".zloop/state.json", "w"), ensure_ascii=False, indent=2)
PY
  )
  echo "$W"
}

PATH_FOR() { echo "$1/bin:$(dirname "$ZLOOP"):$PATH"; }

# 等 runner 自己退出，最多 $2 秒；回报它是停了还是还活着
watch_runner() {
  pid=$1; limit=$2; i=0
  while [ "$i" -lt "$limit" ]; do
    kill -0 "$pid" 2>/dev/null || return 0
    i=$((i+1)); sleep 1
  done
  kill -TERM "$pid" 2>/dev/null; sleep 1; kill -KILL "$pid" 2>/dev/null
  return 1
}

report() { # $1=工作目录
  fails=$(grep -o '"outcome": *"fail"' "$1/.zloop/state.json" 2>/dev/null | wc -l | tr -d " ")
  rounds=$(grep -o '"event":"begin"' "$1/.zloop/runner/journal.jsonl" 2>/dev/null | wc -l | tr -d " ")
  wb=$(grep -o '"wrote_back":[a-z]*' "$1/.zloop/runner/journal.jsonl" 2>/dev/null | sed 's/"wrote_back"://' | tr '\n' ' ')
  echo "  起了 ${rounds} 轮 ｜ 账本里 fail：${fails} 条 ｜ journal 每轮的 wrote_back：${wb}"
}

echo "=== 场景 A：宿主每轮都失败，没人插话 ==="
WA=$(setup); cd "$WA" || exit 2
PATH=$(PATH_FOR "$WA"); export PATH
"$ZLOOP" run --host claude --fast --no-replan --timeout-min 30 >runA.log 2>&1 &
PIDA=$!
if watch_runner $PIDA 20; then
  A=$(grep -o 'runner: stop (.*)' runA.log | tail -1)
  [ -n "$A" ] || A="退出了（没打印 stop）"
else
  A="还在跑（20 秒都没停）"
fi
report "$WA"
echo "  runner: $A"

echo
echo "=== 场景 B：一模一样，只是人每隔 1 秒敲一句 zloop feedback ==="
WB=$(setup); cd "$WB" || exit 2
PATH=$(PATH_FOR "$WB"); export PATH
(
  i=0
  while [ $i -lt 20 ]; do
    "$ZLOOP" feedback t1 "人在另一个终端说：先别动 x.rs" >/dev/null 2>&1
    i=$((i+1)); sleep 1
  done
) &
POKER=$!
"$ZLOOP" run --host claude --fast --no-replan --timeout-min 30 >runB.log 2>&1 &
PIDB=$!
if watch_runner $PIDB 20; then
  B=$(grep -o 'runner: stop (.*)' runB.log | tail -1)
  [ -n "$B" ] || B="退出了（没打印 stop）"
else
  B="还在跑（20 秒都没停）"
fi
kill $POKER 2>/dev/null
report "$WB"
echo "  runner: $B"
grep -o 'runner: round [0-9]* [^·]*' runB.log | head -3 | sed 's/^/  runB.log: /'

echo
case "${A}|${B}" in
  *"fail_streak"*"|"*"还在跑"*)
    echo "[FAIL] 复现成功：同一个失败的宿主，人插一句 feedback 就让 runner 从「${A}」变成「${B}」"
    echo "       runner 把人写的那条 tick 当成了宿主的写回：一条 fail 都没记，fail_streak 恒为 0。"
    exit 1;;
  *"fail_streak"*"|"*"fail_streak"*)
    echo "[OK] 两边都停在 fail_streak：交互式命令写的 tick 不再被当成宿主的写回"
    exit 0;;
  *)
    echo "[?] 结果不在预期的两种里（A=${A} / B=${B}）——先看 ${WA}/runA.log 和 ${WB}/runB.log"
    exit 2;;
esac
