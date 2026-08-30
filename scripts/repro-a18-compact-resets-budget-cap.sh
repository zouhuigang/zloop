#!/bin/sh
# A-18 复现：`zloop compact`（整理账本，默认 --keep-days 7）会把老 todo 的 tick 一起归档走，
# 而**花费就记在 tick 上**。于是一个已经撞到 `policy.max_total_usd` 停下的目标，
# 被整理一次之后花费归零、`decide` 回到 ready、`zloop start` 又肯起来了。
#
# 和 A-16 / A-17 同一类，只是方向反过来：那两条是交互式命令**写进**账本串了调度，
# 这条是交互式命令**从账本里删掉**东西串了调度。runner 读的所有累计量
# （spent_usd / fail_streak / progress_streak / window_ticks）都是从 ticks 现算的，
# 谁动了 ticks 谁就动了这些闸。
#
# 钱这一项最要命：`max_total_usd` 是「这个目标一共只准花这么多」，
# 而 compact 把它变成了「最近 7 天只准花这么多」——一次手滑的整理就是一次静默提额，
# 连 `zloop status` 都不再显示花过的钱。
#
#   sh scripts/repro-a18-compact-resets-budget-cap.sh
#
# 退出码 1 = 复现成功（整理之后预算闸失效）；0 = 修好了。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

W=$(mktemp -d "/tmp/zloop-a18.XXXXXX") || exit 2
cd "$W" || exit 2
"$ZLOOP" init "a18 repro" >/dev/null
"$ZLOOP" plan --add "[P0] 一个月前做完的活" --add "[P1] 还没做的活" >/dev/null

# 造一个跑了一个月、已经花超上限的目标：t1 一个月前完成，那一轮花了 $9.50，上限 $5.00
python3 - <<'PY'
import json, datetime
p = ".zloop/state.json"
s = json.load(open(p))
old = (datetime.datetime.now().astimezone() - datetime.timedelta(days=30)).isoformat()
s["policy"]["max_total_usd"] = 5.0
s["todos"][0].update(status="done", done_at=old, updated_at=old)
s["ticks"] = [{"at": old, "round": 1, "todo": "t1", "outcome": "done", "note": "老活做完",
               "host": "claude", "session": "s1", "log": None, "cost_usd": 9.5,
               "duration_ms": 1000, "num_turns": 3, "documented": True,
               "pitfalls": [], "rethink": None}]
json.dump(s, open(p, "w"), ensure_ascii=False, indent=2)
PY

decision() { "$ZLOOP" next --peek --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print("should_run=%s reason=%s" % (d["should_run"], d["reason"]))'; }

echo "=== 整理之前 ==="
echo "  $(decision)"
"$ZLOOP" status 2>/dev/null | grep -F '$' | head -1 | sed 's/^/ /'
"$ZLOOP" start --host claude --fast 2>&1 | head -1 | sed 's/^/  $ zloop start → /'

echo
echo "=== 人做了一次例行整理：zloop compact（默认 --keep-days 7） ==="
"$ZLOOP" compact | sed 's/^/  /'

echo
echo "=== 整理之后 ==="
AFTER=$(decision)
echo "  ${AFTER}"
"$ZLOOP" status 2>/dev/null | grep -F '$' | head -1 | sed 's/^/ /'
echo "  （上面这行如果不再提花过的钱，就是账已经没了——归档文件里还有，但没人再读它）"

echo
case "${AFTER}" in
  *"reason=budget"*)
    echo "[OK] 整理不再抹掉花费：预算闸还在"
    exit 0;;
  *)
    echo "[FAIL] 复现成功：整理之前 reason=budget（已停），整理之后 ${AFTER}"
    echo "       policy.max_total_usd 被静默复位，长跑可以接着烧钱。工作目录：${W}"
    exit 1;;
esac
