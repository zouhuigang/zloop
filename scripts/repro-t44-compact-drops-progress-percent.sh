#!/bin/sh
# T44 复现：`zloop compact`（整理账本）把老 todo 搬进 `archive/`，而「这个目标做到哪儿了」
# 是**从 state.todos 现数出来的**。T29 修的是从 ticks 现算的那一族（轮次/返工/花费），
# 从 todo 现算的这一族当时留着没动：
#
#   * `zloop status` 的进度条和百分比：66% → **0%**（分子分母一起被搬走）；
#   * `zloop stats` 的「一次过 X/Y 条」：2/2 → **0/0**——同一张表上面写着「跑了 2 轮」，
#     下面说一条都没完成过；
#   * `zloop goals` 那张表里这个目标的进度：2/3 → **0/1**。
#
# 口径（这一条 todo 要定的就是它）：百分比、一次过、goals 的进度回答的是
# **「这个目标做到哪儿了」**，和同一行的「跑了 N 轮」「花了 $X」一样是一辈子的账，
# 归档走的 todo 得算进去；「还剩几步」「步骤 1..N」只讲账本里还剩的，那是 compact 的本意。
# 两个口径同屏，靠 `归档` 那行说出来。
#
#   sh scripts/repro-t44-compact-drops-progress-percent.sh
#
# 退出码 1 = 复现成功（整理之后进度掉了）；0 = 修好了。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

W=$(mktemp -d "/tmp/zloop-t44.XXXXXX") || exit 2
cd "$W" || exit 2
"$ZLOOP" init "t44 repro" >/dev/null
"$ZLOOP" plan --add "[P0] 老活一" --add "[P0] 老活二" --add "[P1] 还没做的活" >/dev/null

# 两条完成（都是一次过），一条还没做 —— 进度 2/3 = 66%
"$ZLOOP" done t1 --note "ok" --no-doc >/dev/null
"$ZLOOP" done t2 --note "ok" --no-doc >/dev/null

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
  "$ZLOOP" status 2>/dev/null | grep -F '跑了'   | sed 's/^/  status  /'
  # 只要「质量」那一行：下面的清单里每条一次过的 todo 也带这三个字
  "$ZLOOP" stats  2>/dev/null | grep -F '一次过' | head -1 | sed 's/^ */  stats   /'
  # 第三处：`goals` 那张表里的 done/total —— 同一个问题的第三个出口
  "$ZLOOP" goals  2>/dev/null | grep -F '▸ '    | sed 's/^ */  goals   /'
}

echo "=== 整理之前 ==="
show

echo
echo "=== 人做了一次例行整理：zloop compact --keep-days 30 ==="
"$ZLOOP" compact --keep-days 30 | head -1 | sed 's/^/  /'

echo
echo "=== 整理之后 ==="
show
AFTER=$("$ZLOOP" stats --json 2>/dev/null |
  python3 -c 'import json,sys; d=json.load(sys.stdin); print("%d/%d" % (d["first_try"], d["done"]))')
PCT=$("$ZLOOP" status 2>/dev/null | grep -F '跑了' | grep -oE '[0-9]+%' | head -1)

echo
case "${PCT} ${AFTER}" in
  "66% 2/2")
    echo "[OK] 整理不再抹掉进度：66% / 一次过 2/2 都还在"
    exit 0;;
  *)
    echo "[FAIL] 复现成功：整理之前 66% / 一次过 2/2，整理之后 ${PCT} / 一次过 ${AFTER}"
    echo "       「这个目标做到哪儿了」被一次例行整理清零。工作目录：${W}"
    exit 1;;
esac
