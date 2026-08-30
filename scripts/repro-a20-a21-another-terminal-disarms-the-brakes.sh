#!/bin/sh
# A-20 / A-21 复现：A-17 修完之后，同一张表上还坐着两条**一模一样形状**的——
# 「人在另一个终端敲一条交互式命令，就把无头 runner 的停机闸拆了」。
#
#   A-20  `zloop edit` 无条件清 `fail_streak`（tick.rs `fails_in_a_row` 的 `_ => n = 0`）。
#         edit tick 只有 `zloop edit` 会记（cli.rs:1033），也就是**人敲的**。
#         改的哪怕是**另一条毫不相干的 todo**（顺手整理 backlog、把 t7 推后），
#         正在失败的那条活的连续失败计数照样归零。
#
#   A-21  `zloop feedback` 无条件清 `progress_streak`（tick.rs `progress_streak` 的 `_ => break`）。
#         这就是 A-17 后半截在 `fail_streak` 上修掉的那个形状，只是换了一条 streak：
#         宿主每轮都 `--outcome progress` 原地踏步，人每隔一会儿补一句反馈，
#         `max_progress_streak` 这道闸就永远数不到上限。
#
#   sh scripts/repro-a20-a21-another-terminal-disarms-the-brakes.sh
#
# 退出码 1 = 至少一条复现成功；0 = 两条都修好了（四个场景都停）。
#
# 环境变量：ZLOOP=<二进制路径>
set -u
ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null || echo .)
ZLOOP=${ZLOOP:-$ROOT/target/debug/zloop}
case "$ZLOOP" in /*) ;; *) ZLOOP="$PWD/$ZLOOP" ;; esac

[ -x "$ZLOOP" ] || { echo "找不到 $ZLOOP，先 cargo build --bin zloop"; exit 2; }

# $1 = fail | progress —— 假宿主的两种脾气
setup() {
  W=$(mktemp -d "/tmp/zloop-a20.XXXXXX") || exit 2
  mkdir -p "$W/bin"
  if [ "$1" = fail ]; then
    # 每轮都失败：不写回、非零退出、慢到人来得及插话
    cat > "$W/bin/claude" <<'EOF'
#!/bin/sh
sleep 2
echo "host blew up" >&2
exit 1
EOF
  else
    # 每轮都「有进展没做完」：真写回，但同一条 todo 永远完不了
    cat > "$W/bin/claude" <<EOF
#!/bin/sh
sleep 1
"$ZLOOP" done t1 --outcome progress --note "又推进了一点，还没完" \
  --approach "把大的一步切一半" >/dev/null 2>&1
EOF
  fi
  chmod +x "$W/bin/claude"
  ( cd "$W" || exit 2
    "$ZLOOP" init "a20/a21 repro" >/dev/null
    "$ZLOOP" plan --add "[P0] 干点活" >/dev/null
    "$ZLOOP" plan --add "[P1] 另一条毫不相干的活" >/dev/null
    python3 - <<'PY'
import json
s = json.load(open(".zloop/state.json"))
s["policy"]["max_fail_streak"] = 2        # 连着 2 轮失败就该停
s["policy"]["max_progress_streak"] = 2    # 同一条 todo 连着 2 轮 progress 就该停
s["policy"]["intervals_min"] = [1, 1, 2]  # --fast 下按秒算
json.dump(s, open(".zloop/state.json", "w"), ensure_ascii=False, indent=2)
PY
  )
  echo "$W"
}

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
  progs=$(grep -o '"outcome": *"progress"' "$1/.zloop/state.json" 2>/dev/null | wc -l | tr -d " ")
  rounds=$(grep -o '"event":"begin"' "$1/.zloop/runner/journal.jsonl" 2>/dev/null | wc -l | tr -d " ")
  echo "  起了 ${rounds} 轮 ｜ 账本里 fail：${fails} 条 ｜ progress：${progs} 条"
}

# $1=模式(fail|progress) $2=戳法(none|edit|feedback) $3=标题 → 结果写进 $RESULT
scenario() {
  echo "=== $3 ==="
  W=$(setup "$1"); cd "$W" || exit 2
  PATH="$W/bin:$(dirname "$ZLOOP"):$PATH"; export PATH
  POKER=
  if [ "$2" != none ]; then
    (
      i=0
      while [ $i -lt 25 ]; do
        case "$2" in
          # 人顺手整理 backlog：改的是 t2，跟正在失败的 t1 没有半点关系
          edit)     "$ZLOOP" edit t2 --text "另一条毫不相干的活（整理第 $i 次）" >/dev/null 2>&1 ;;
          feedback) "$ZLOOP" feedback t1 "人在另一个终端说：先别动 x.rs" >/dev/null 2>&1 ;;
        esac
        i=$((i+1)); sleep 1
      done
    ) &
    POKER=$!
  fi
  "$ZLOOP" run --host claude --fast --no-replan --timeout-min 30 >run.log 2>&1 &
  PID=$!
  if watch_runner $PID 20; then
    RESULT=$(grep -o 'runner: stop (.*)' run.log | tail -1)
    [ -n "$RESULT" ] || RESULT="退出了（没打印 stop）"
  else
    RESULT="还在跑（20 秒都没停）"
  fi
  # 花括号不能省：bash-as-sh 在非 UTF-8 locale 下会把紧跟着的中文字节当成变量名的一部分
  [ -n "${POKER}" ] && { kill "${POKER}"; wait "${POKER}"; } 2>/dev/null
  report "$W"
  echo "  runner: $RESULT"
  echo
}

scenario fail none "A-20 场景 A：宿主每轮都失败，没人插话"; A=$RESULT
scenario fail edit "A-20 场景 B：一模一样，只是人每隔 1 秒 zloop edit t2（另一条 todo！）"; B=$RESULT
scenario progress none "A-21 场景 C：宿主每轮都 progress 原地踏步，没人插话"; C=$RESULT
scenario progress feedback "A-21 场景 D：一模一样，只是人每隔 1 秒 zloop feedback t1"; D=$RESULT

bad=0
echo "--- 判定 ---"
case "$A|$B" in
  *fail_streak*"|"*fail_streak*) echo "[OK]   A-20：改别人的 todo 不再拆掉连续失败这道闸（A=${A} / B=${B}）";;
  *fail_streak*"|"*)             echo "[FAIL] A-20 复现：同一个必败的宿主，人在另一个终端改了**别的** todo，runner 就从「${A}」变成「${B}」"; bad=1;;
  *)                             echo "[?]    A-20 场景 A 就没停在 fail_streak（A=${A}），先看日志"; bad=2;;
esac
case "$C|$D" in
  *progress_streak*"|"*progress_streak*) echo "[OK]   A-21：反馈不再拆掉原地踏步这道闸（C=${C} / D=${D}）";;
  *progress_streak*"|"*)                 echo "[FAIL] A-21 复现：同一个原地踏步的宿主，人插一句 feedback，runner 就从「${C}」变成「${D}」"; bad=1;;
  *)                                     echo "[?]    A-21 场景 C 就没停在 progress_streak（C=${C}），先看日志"; [ "$bad" = 0 ] && bad=2;;
esac
exit $bad
