#!/bin/sh
# A-19 复现：人在自己的 Claude Code 会话里敲一句 `zloop feedback`（或 `zloop edit`），
# 下一轮 runner 就会拿**人的那个会话 id** 去 `claude -p --resume`——无头轮次跑进了
# 人正开着的对话里。
#
# 又是同一类：交互式命令写进账本的东西串进了 runner 的判断。这次串进去的不是停机判断，
# 是「这一轮 resume 谁」（runner.rs::pick_session）：
#
#     let same_host = |t| t.host == Some(host) && t.session.is_some();
#     ResumeMode::Todo => ticks.rev().filter(same_host).find(|t| t.todo == Some(todo_id))
#
# 它只看 host 和 todo，**不看这条 tick 是谁记的**。而 `zloop feedback` / `zloop edit`
# 会把调用者的 `CLAUDE_CODE_SESSION_ID` 原样记进 tick（`session::detect()`），
# 于是「人在交互会话里给这条 todo 留句话」= 「把自己的会话 id 挂到这条 todo 名下」。
#
# 后果不只是记错：`--resume` 上去之后这一轮的提示词接在人那段对话后面跑——
# 上下文全是不相干的，token 按整段转录计费，产出还写进人的转录里。
#
#   sh scripts/repro-a19-runner-resumes-a-humans-session.sh
#
# 退出码 1 = 复现成功（runner resume 了人的会话）；0 = 修好了。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

W=$(mktemp -d "/tmp/zloop-a19.XXXXXX") || exit 2
mkdir -p "$W/bin"
# 假宿主：把 runner 传给它的 `--resume <id>` 记下来，然后正常写回
cat > "$W/bin/claude" <<'EOF'
#!/bin/sh
sid=""; prev=""
for a in "$@"; do [ "$prev" = "--resume" ] && sid="$a"; prev="$a"; done
echo "$sid" >> "$ZLOOP_A19_LOG"
id=$(printf "%s" "$2" | sed -n "s/.*当前 todo：\(t[0-9]*\) .*/\1/p" | head -1)
zloop done "$id" --note ok --approach "假宿主" >/dev/null 2>&1
echo '{"session_id":"HOST-SESSION-REAL","is_error":false,"result":"ok"}'
EOF
chmod +x "$W/bin/claude"

cd "$W" || exit 2
"$ZLOOP" init "a19 repro" >/dev/null
"$ZLOOP" plan --add "[P0] 第一件事" --add "[P1] 第二件事" >/dev/null
python3 - <<'PY'
import json
s = json.load(open(".zloop/state.json"))
s["policy"]["intervals_min"] = [1, 1, 2]  # --fast 下按秒算
json.dump(s, open(".zloop/state.json", "w"), ensure_ascii=False, indent=2)
PY
PATH="$W/bin:$(dirname "$ZLOOP"):$PATH"; export PATH
ZLOOP_A19_LOG="$W/resume.log"; export ZLOOP_A19_LOG
: > "$ZLOOP_A19_LOG"

echo "=== 人在自己的 Claude Code 会话（HUMAN-SESSION-9999）里给 t1 留了一句反馈 ==="
CLAUDE_CODE_SESSION_ID="HUMAN-SESSION-9999" "$ZLOOP" feedback t1 "顺便说一句：先别动 x.rs" | head -1 | sed 's/^/  $ zloop feedback → /'

echo "=== runner 无头跑两轮（--resume 默认就是 todo） ==="
"$ZLOOP" run --host claude --fast --no-replan --max-rounds 2 2>&1 | grep -E "round [0-9]+ →|stop" | sed 's/^/  /'

echo "=== 假宿主实际收到的 --resume ==="
n=0
while IFS= read -r line; do
  n=$((n+1))
  echo "  第 ${n} 轮：--resume [${line}]"
done < "$ZLOOP_A19_LOG"

echo
if grep -q "HUMAN-SESSION-9999" "$ZLOOP_A19_LOG"; then
  echo "[FAIL] 复现成功：无头的第一轮跑进了人的对话里（--resume HUMAN-SESSION-9999）"
  echo "       人只是留了句反馈，从没让 runner 接管自己的会话。工作目录：${W}"
  exit 1
fi
echo "[OK] runner 没有 resume 人的会话：交互式命令留下的 session id 不再被当成上一轮的宿主会话"
exit 0
