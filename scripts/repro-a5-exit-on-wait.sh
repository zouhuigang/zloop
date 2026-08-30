#!/bin/sh
# A-5 复现：`--exit-on-wait` 在「等人」时不生效——runner 一直按最慢间隔转下去。
#
# 关键是**全程走真实路径**：init → plan → runner 起跑 → 宿主自己 `zloop done --block`
# 把 todo 挂到 user_gate 上 → 下一轮 runner 撞上 user_gate。中间一次手工搓状态都没有
# （原来的测试靠先敲 3 次 `zloop next` 把 noop_streak 顶满，才让 decide() 返回
# interval=None——而真 runner 在 !should_run 那一支只记 journal 的 sleep，一条 noop
# 都不记，所以这条路它自己永远走不到）。
#
#   sh scripts/repro-a5-exit-on-wait.sh
#
# 退出码 1 = 复现成功（带着 --exit-on-wait 却还在转）；0 = 它按说明书退出了（修好了就该是 0）。
#
# 环境变量：ZLOOP=<二进制路径> WATCH=<观察多少秒>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}
WATCH=${WATCH:-15}            # --fast 下间隔按秒算，15 秒足够转好几圈

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

W=$(mktemp -d "/tmp/zloop-a5.XXXXXX") || exit 2
cd "$W" || exit 2

# 假宿主：第一轮把这条 todo 交回给人（--block 是宿主协议里的正规动作），然后秒退。
mkdir -p bin
cat > bin/claude <<'EOF'
#!/bin/sh
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note "要人拍板" --approach "假宿主" --block "用哪个库？" >/dev/null 2>&1
echo '{"session_id":"a5","is_error":false,"result":"blocked"}'
EOF
chmod +x bin/claude
PATH="$W/bin:$(dirname "$ZLOOP"):$PATH"; export PATH

"$ZLOOP" init "a5 repro" >/dev/null
"$ZLOOP" plan --add "[P0] 需要人拍板" >/dev/null

echo "[setup] $W · 宿主第一轮就 --block，第二轮起 runner 撞 user_gate"

# --max-rounds 0 = 不自己收工；说明书说带 --exit-on-wait 就该在 user_gate 上退出。
"$ZLOOP" run --host claude --fast --exit-on-wait --no-replan > run.log 2>&1 &
PID=$!

i=0
while [ "$i" -lt "$WATCH" ]; do
  kill -0 "$PID" 2>/dev/null || break
  i=$((i + 1))
  sleep 1
done

if kill -0 "$PID" 2>/dev/null; then
  kill -TERM "$PID" 2>/dev/null
  sleep 2
  kill -KILL "$PID" 2>/dev/null
  SLEEPS=$(grep -c '"event":"sleep"' .zloop/runner/journal.jsonl 2>/dev/null | head -1)
  NOOPS=$(grep -o '"outcome":"noop"' .zloop/state.json 2>/dev/null | wc -l | tr -d ' ')
  echo "[FAIL] 带着 --exit-on-wait 转了 ${WATCH}s 还活着：journal 里 $SLEEPS 条 sleep，账本里 $NOOPS 条 noop"
  echo "---- run.log 末尾 ----"; tail -5 run.log
  exit 1
fi

wait "$PID"; CODE=$?
REASON=$(tail -3 run.log | tr -d '\n')
if grep -q "runner: stop (user_gate)" run.log; then
  echo "[OK] runner 在 user_gate 上退出了（exit=${CODE}）：$REASON"
  exit 0
fi
echo "[FAIL] 进程没了但不是因为 user_gate（exit=${CODE}）：$REASON"
exit 1
