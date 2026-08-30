# 开发

> 怎么构建、测试、跑格式闸，以及写文档的约定。
>
> ← 回到 [README](../../README.md)

```bash
sh scripts/check.sh              # 整道闸：fmt --check → clippy -D warnings → cargo test（约 2 分钟）
sh scripts/check.sh fmt clippy   # 只跑便宜的前两道（秒级，改完随手过一下）
cargo build --release && install -m755 target/release/zloop ~/.local/bin/zloop
```

**闸只有一份定义。** `scripts/check.sh` 就是 CI（`.github/workflows/ci.yml`）跑的那个文件——
本地过了和 CI 过了永远是同一句话。`tests/gate_test.rs` 把这条钉成断言：workflow 里不许把
`cargo fmt --all` / `-D warnings` / `cargo test` 再抄一遍，只能去调这个脚本。

**CI 跑在 macOS 上，不是 ubuntu。** `awake::supported()` 是 `cfg!(target_os = "macos")`，
非 macOS 上整个 keep-awake 层是 no-op，而 7 个测试断言的正是 `pmset` 真的被调过——
在 ubuntu 上跑等于开局 7 红。工具链跟着 runner 镜像的 stable 走（没 pin），
新版 clippy 加一条 lint 会让 CI 突然变红；那时候的处置是修掉或 `allow` 掉，不是摘掉 `-D warnings`。

想让 runner 每轮开跑前也过一道，在 `.zloop/state.json` 的 `policy` 里写
`"preflight_cmd": "sh scripts/check.sh fmt clippy"`。
**别把 `test` 也塞进去**：preflight 失败会记 `fail` 而不调宿主，连红 `max_fail_streak` 轮就停机——
一棵红树会把整个长跑卡死在"走不到修好它那一步"。

**格式：`rustfmt.toml` 是必须的，别删。** 这份代码是 ~125 列的密排风格，rustfmt 的两个默认值都跟它对不上：

| 默认值 | 后果 | 本仓库 |
|---|---|---|
| `max_width = 100` | 29 个文件全判成不合规 | `max_width = 125` |
| `use_small_heuristics = "Default"`（`fn_call_width=60` / `chain_width=60` / `struct_lit_width=18`…） | 没超宽的调用和结构体字面量也被拆行，凭空多出 ~2400 行 | `use_small_heuristics = "Max"`，只有真超 125 列才换行 |

没有这份配置，`cargo fmt --check` 在全仓所有文件上都是红的——那不叫格式闸，那叫没有闸。
配上之后 `cargo fmt --check` 退 0，改动才有一道机械的格式基线。
一次性对齐的那个提交只动格式、不动语义，已记进 `.git-blame-ignore-revs`；想让 `git blame` 跳过它：

```bash
git config blame.ignoreRevsFile .git-blame-ignore-revs
```

注意 rustfmt 按**字符**数算宽度，不按终端列宽。这份代码里中文注释很多，一个汉字占 2 列却只算 1 个字符，
所以含中文的行显示出来会比 125 列宽——这是 rustfmt 的已知限制，好处是它不会去拆中文注释行。

目录：`src/` 实现 · `tests/` 集成测试 · `docs/`：[`FINDINGS.md`](../../docs/audit/FINDINGS.md) 确认缺陷清册（**按缺陷查看这一份**）、[`CODE-AUDIT.md`](../../docs/audit/CODE-AUDIT.md) 全量代码审查正文（按轮次读）、`RUST-DESIGN.md` 当前设计、`LONG-RUN-AUDIT.md` 长程加固审计、`OPEN-SOURCE-REVIEW.md` 开源方案对照与借鉴、`TEST-REPORT.md` 自测报告、`loopx-principles.md` / `loopx-scheduling-notes.md` loopx 研究、`DESIGN.md` v0.1 Python 原型设计记录。

文档里的跨文件链接和锚点由 `python3 scripts/check-doc-links.py` 守着（`sh scripts/check.sh` 的第一道，CI 也跑）：
链接指向不存在的文件或对不上的标题就红。节号还多两条约束——**不重复、不跳号、第一节只能是 `§0` 或 `§1`**，
以及**每个 `§N` 都得指得到**（先判这个号说的是哪份文档：待在链接里就按链接指向的那份，
否则看同一行前面提到的 `xxx.md`，都没有才算自指），因为整份清册靠「正文 §N」把人送回正文，
而这种引用**没有编译器**，写歪了不会有任何东西报错。这两条 t46 之前只管 `CODE-AUDIT.md` 和
`FINDINGS.md`，现在覆盖 README + `docs/` 全部 15 份，共 166 处 `§N`。
闸自己也有回归测试：`--self-test` 拿一组合成文档跑一遍，报出来的必须**正好**是期待的那 10 条
（少一条 = 规则失灵，多一条 = 规则误伤），`cargo test` 里的 `the_doc_link_gate_rules_still_bite` 调的就是它。

### 写文档的一条约定：跨文档引用一律写成锚点链接

**指别的文档的某一节，写 `[xxx.md §N](xxx.md#锚点)`，不要写 `xxx.md §N`**（形如「见 `notes.md §3.3`」的裸号）。
理由是这两种写法被验的程度差一大截：

| 写法 | 归属（说的是哪份文档） | 落点（点进去到哪儿） |
|---|---|---|
| 裸号 `notes.md §3.3` | 闸靠「同一行、§ 之前最近提到的 `.md`」**猜**——猜错了不会有任何东西报错 | 没有落点，读的人自己去翻 |
| 锚点链接 | 就是链接指向的那份，不用猜 | R1 验锚点真的存在，R3a 验「文字里写的号」和「锚点落的那一节」是同一节 |

这条从 t47 起由 **R5** 机械执行：`§N` 指的是本仓库另一份文档、又没待在链接里，就红。
三级归属退回它本来的位置——**兜底**，只留给写不成链接的那一种：目标文档不在这个仓库里
（loopx 上游的 `field-derived-patterns.md` 之类，共 4 处，跳过并计数）。
立规矩当天全仓有 7 处裸的跨文档引用（README 1 行 2 处、`CODE-AUDIT.md` 1 处、
`LONG-RUN-AUDIT.md` 1 处、`loopx-principles.md` 3 处），已全部改成链接；
`§N` 里待在链接中的从 90 处变成 97 处，裸的跨文档引用归零。

两个不受影响的写法：**自指**的 `§N`（同一份文档里前后引用，69 处）照旧裸着写；
`` `§7.1` `` 这种**待在行内代码里**的是在引用写法本身，不查——包括上面这张表里的例子。

MIT License.
