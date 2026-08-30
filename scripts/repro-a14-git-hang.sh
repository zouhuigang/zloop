#!/bin/sh
# A-14 复现：runner 每轮起的 git 子进程（以及 notify_cmd）没有闸。
#
# 这些 `.output()` / `wait_with_output()` 是无限期的阻塞等待：既不看 `--timeout-min`，
# 也不看 `stop_requested()`。git 一挂住（钩子、fsmonitor、网络文件系统 stall），
# runner 就跟着挂住，`zloop stop` 的 SIGTERM 叫不动它——和 A-6 是同一类死法，换了条路。
#
#   sh scripts/repro-a14-git-hang.sh commit   # 卡在 git_checkpoint 的 git commit 上（pre-commit 钩子）
#   sh scripts/repro-a14-git-hang.sh status   # 卡在开工前的 git_dirty 上（core.fsmonitor 钩子）
#   sh scripts/repro-a14-git-hang.sh notify   # 卡在收尾通知的 notify_cmd 上
#
# 退出码 1 = 复现成功（runner 卡住且 SIGTERM 叫不动）；0 = 没卡住（修好了就该是 0）。
#
# 环境变量：ZLOOP=<二进制路径> HANG=<钩子挂多少秒> SIG_AT=<第几秒发 SIGTERM> WAIT_AFTER=<等几秒>
set -u
WHERE=${1:-commit}
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}
HANG=${HANG:-987}             # 987 而不是 60/600：好让 pkill -f 精确匹配，不误伤别人的 sleep
SIG_AT=${SIG_AT:-12}
WAIT_AFTER=${WAIT_AFTER:-10}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

W=$(mktemp -d "/tmp/zloop-a14-$WHERE.XXXXXX") || exit 2
cd "$W" || exit 2

# 假宿主：写个文件 + 写回，秒退。真正慢的只有 git。
mkdir -p bin
cat > bin/claude <<'EOF'
#!/bin/sh
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
echo "work for $id" > "$id.txt"
zloop done "$id" --note "wrote $id.txt" --approach "fake host" >/dev/null 2>&1
echo '{"session_id":"a14","is_error":false,"result":"ok"}'
EOF
chmod +x bin/claude
PATH="$W/bin:$(dirname "$ZLOOP"):$PATH"; export PATH

"$ZLOOP" init "a14 repro" >/dev/null
"$ZLOOP" plan --add "[P0] a" >/dev/null
git init -q -b main .; git config user.email t@e.com; git config user.name t
printf '.zloop/\nbin/\n' > .gitignore
git add -A; git commit -qm init

mkdir -p .git/hooks
case "$WHERE" in
  commit)  # 真实来源：husky / lefthook / pre-commit 跑的检查卡在网络或一把锁上
    printf '#!/bin/sh\nsleep %s\n' "$HANG" > .git/hooks/pre-commit
    chmod +x .git/hooks/pre-commit
    echo "[setup] pre-commit 挂 ${HANG}s → 卡在 git_checkpoint 的 commit 上" ;;
  status)  # 真实来源：fsmonitor 钩子 / 网络文件系统 stall，git status 读不动工作树
    printf '#!/bin/sh\nsleep %s\nprintf "/\\0"\n' "$HANG" > .git/hooks/fsmonitor-slow
    chmod +x .git/hooks/fsmonitor-slow
    git config core.fsmonitor .git/hooks/fsmonitor-slow
    echo "[setup] core.fsmonitor 挂 ${HANG}s → 卡在开工前的 git_dirty 上" ;;
  notify)  # 真实来源：notify_cmd 里的 osascript 弹窗 / 没有 -m 的 curl / 发不出去的 mail
    python3 - "$HANG" <<'PY'
import json, sys
p = ".zloop/state.json"
s = json.load(open(p)); s["policy"]["notify_cmd"] = "sleep " + sys.argv[1]
json.dump(s, open(p, "w"), ensure_ascii=False, indent=2)
PY
    echo "[setup] notify_cmd = sleep ${HANG}s → 卡在收尾那一下的通知上（活全干完了却退不出去）" ;;
  *) echo "用法：$0 [commit|status|notify]"; exit 2 ;;
esac

# --timeout-min 5 --fast = 这一轮的闸是 5 秒；正常情况整个 run 两三秒就结束。
# 闸不能压到 1 秒：假宿主里那个 `zloop done` 自己就要几百毫秒，机器一忙就误判超时，
# 那一轮 wrote_back=false，根本走不到 checkpoint——白跑一趟还看不出来。
EXTRA="--git-commit --max-rounds 1"
# notify 那条路不需要 git，也**不能**加 --max-rounds：`stop()` 对 max_rounds 和 sigterm
# 这两个理由不发通知，加了就永远走不到那一下。让它自然跑成 done。
[ "$WHERE" = notify ] && EXTRA=""
"$ZLOOP" run --host claude --fast $EXTRA --timeout-min 5 > run.log 2>&1 &
PID=$!
S=$(date +%s)
el() { echo $(( $(date +%s) - S )); }

while [ "$(el)" -lt "$SIG_AT" ]; do
  kill -0 $PID 2>/dev/null || break
  sleep 1
done

if ! kill -0 $PID 2>/dev/null; then
  wait $PID
  echo "RESULT: ✅ runner 在 $(el)s 正常退出 —— 没卡住"
  sed 's/^/  | /' run.log
  pkill -f "sleep $HANG" 2>/dev/null
  echo "W=$W"; exit 0
fi

echo "[t=$(el)s] runner 还在跑（这一轮的闸是 5 秒），发 SIGTERM —— 等价于 zloop stop / 关机"
kill -TERM $PID
T=$(date +%s)
while [ $(( $(date +%s) - T )) -lt "$WAIT_AFTER" ]; do
  kill -0 $PID 2>/dev/null || break
  sleep 1
done

RC=0
if kill -0 $PID 2>/dev/null; then
  echo "RESULT: ❌ SIGTERM 之后 ${WAIT_AFTER}s，runner (pid $PID) 还活着 —— 只剩 SIGKILL"
  ps -o pid,etime,command -p $PID | tail -1
  pgrep -P $PID | while read -r c; do ps -o pid,etime,command -p "$c" | tail -1; done
  kill -9 $PID 2>/dev/null
  sleep 1
  # SIGKILL 之后的余波：git 子进程成了孤儿，还攥着 .git/index.lock
  if [ -e .git/index.lock ]; then
    echo "余波: .git/index.lock 还在 —— 这个仓库后面所有 git 写操作都会失败："
    git add -A 2>&1 | head -1 | sed 's/^/  /'
  fi
  RC=1
else
  echo "RESULT: ✅ SIGTERM 之后 $(( $(date +%s) - T ))s 内退出"
fi
wait $PID 2>/dev/null
echo "--- run.log ---";  sed 's/^/  | /' run.log
echo "--- journal ---";  sed 's/^/  | /' .zloop/runner/journal.jsonl 2>/dev/null
echo "--- git log ---";  git log --oneline 2>/dev/null | sed 's/^/  | /'
pkill -f "sleep $HANG" 2>/dev/null
echo "W=$W"
exit $RC
