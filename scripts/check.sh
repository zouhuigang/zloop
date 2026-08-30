#!/bin/sh
# 仓库的**唯一一道闸**：格式 + lint + 测试。CI（`.github/workflows/ci.yml`）跑的就是这个文件，
# 人在本地敲的也是这个文件——闸只有一份定义，CI 和本地不会各写一套然后慢慢跑偏。
#
#   sh scripts/check.sh              # 三道全过
#   sh scripts/check.sh fmt clippy   # 只跑前两道（快，适合当 preflight）
#
# 退出码 0 = 全过；非 0 = 第一道没过的那一道的退出码。
#
# **fail-fast 是故意的**：三道按「越便宜越靠前」排（fmt 秒级 → clippy 一次编译 → test 全跑），
# 红了就地停，不再往下烧时间。它同时是 `policy.preflight_cmd` 的合法取值——在
# `.zloop/state.json` 的 `policy` 里写：
#
#   "preflight_cmd": "sh scripts/check.sh fmt clippy"
#
# 注意 preflight 失败会记一笔 `fail` 而不调宿主，连红 `max_fail_streak` 轮 runner 就停机——
# 想让每轮开跑前先过一道，用便宜的前两道；把 test 也塞进去等于让一棵红树把整个长跑卡死
# （runner 再也走不到「修好它」那一步）。
set -u

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null) || ROOT=$(dirname "$0")/..
cd "$ROOT" || exit 2

GATES=${*:-"fmt clippy test"}

run_gate() {
    printf '\n==> %s\n' "$*"
    "$@" || {
        rc=$?
        printf '\n[check] 没过：%s（退出码 %s）\n' "$*" "$rc"
        exit "$rc"
    }
}

for g in $GATES; do
    case "$g" in
    fmt) run_gate cargo fmt --all -- --check ;;
    # --all-targets 把 tests/ 也算进去：闸不能只看 src/，回归测试才是这仓库的主要产物。
    clippy) run_gate cargo clippy --all-targets --all-features -- -D warnings ;;
    # 光秃秃的 `cargo test`（不加 --all-targets）是故意的：--all-targets 会**跳过 doc test**，
    # 而这仓库的模块头注释里全是给接手人看的说明，doc test 是它们唯一的编译检查。
    test) run_gate cargo test ;;
    *)
        echo "[check] 不认识的闸：$g（可选 fmt / clippy / test）" >&2
        exit 2
        ;;
    esac
done

printf '\n[check] 全过：%s\n' "$GATES"
