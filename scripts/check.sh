#!/bin/sh
# 仓库的**唯一一道闸**：文档链接 + 格式 + lint + 测试。CI（`.github/workflows/ci.yml`）跑的就是这个文件，
# 人在本地敲的也是这个文件——闸只有一份定义，CI 和本地不会各写一套然后慢慢跑偏。
#
#   sh scripts/check.sh              # 四道全过
#   sh scripts/check.sh docs fmt clippy   # 只跑前三道（快，适合当 preflight）
#
# 退出码 0 = 全过；非 0 = 第一道没过的那一道的退出码。
#
# **fail-fast 是故意的**：四道按「越便宜越靠前」排（docs 毫秒级 → fmt 秒级 → clippy 一次编译 → test 全跑），
# 红了就地停，不再往下烧时间。它同时是 `policy.preflight_cmd` 的合法取值——在
# `.zloop/state.json` 的 `policy` 里写：
#
#   "preflight_cmd": "sh scripts/check.sh docs fmt clippy"
#
# 注意 preflight 失败会记一笔 `fail` 而不调宿主，连红 `max_fail_streak` 轮 runner 就停机——
# 想让每轮开跑前先过一道，用便宜的前三道；把 test 也塞进去等于让一棵红树把整个长跑卡死
# （runner 再也走不到「修好它」那一步）。
set -u

ROOT=$(git -C "$(dirname "$0")" rev-parse --show-toplevel 2>/dev/null) || ROOT=$(dirname "$0")/..
cd "$ROOT" || exit 2

GATES=${*:-"docs fmt clippy test"}

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
    # 排最前面因为最便宜（几十毫秒，不用编译）。守的是文档里的跨文件链接、锚点和节号引用：
    # 「正文 §N」这类引用没有编译器，写歪了不会有任何东西报错——t45 立这道闸时
    # 现场就抓到三处腐烂（重复的节号、指错的节、一条本来就坏的锚点），t46 把节号那两条
    # 从两份文档推广到全仓（README + docs/ 全部 15 份、166 处 §N），又抓到三处。
    # t47 加了 R5：跨文档的 §N 不许裸着写，一律写成 `[xxx.md §N](xxx.md#锚点)`——裸号的归属
    # 是闸猜出来的、落点根本没验；写成链接才两样都被验。上任当场抓到 7 处，已全部改完。
    # 闸自己的回归测试是 `--self-test`（合成文档，报出来的必须一条不多一条不少），
    # 它挂在 cargo test 里，不在这一道单跑——这一道要保持「几十毫秒」的性价比。
    docs) run_gate python3 scripts/check-doc-links.py ;;
    fmt) run_gate cargo fmt --all -- --check ;;
    # --all-targets 把 tests/ 也算进去：闸不能只看 src/，回归测试才是这仓库的主要产物。
    clippy) run_gate cargo clippy --all-targets --all-features -- -D warnings ;;
    # 光秃秃的 `cargo test`（不加 --all-targets）是故意的：--all-targets 会**跳过 doc test**，
    # 而这仓库的模块头注释里全是给接手人看的说明，doc test 是它们唯一的编译检查。
    test) run_gate cargo test ;;
    *)
        echo "[check] 不认识的闸：$g（可选 docs / fmt / clippy / test）" >&2
        exit 2
        ;;
    esac
done

printf '\n[check] 全过：%s\n' "$GATES"
