# 全量代码审查（2026-08-29）

> 目标原话是"确保代码没有 bug 和漏洞"。**这做不到**——不存在证明不了。
> 这份文档能给的是：按风险面逐块过一遍，每条发现都带**可复现的失败场景**；
> 试过但复现不出来的，写明"试过没复现"，不写进 issue，免得污染真发现。

> **要查某一条缺陷，先去 [`docs/FINDINGS.md`](../../docs/audit/FINDINGS.md)**（42 条确认缺陷的清册：
> 一览表 + 逐条草稿，每条带锚点直达这边的正文）。这边是**过程**，按审查轮次排，
> 从上往下读；那边是**结果**，按缺陷查。
>
> 编号约定：`§N` ＝ 第 (N−3) 轮（§5 是第二轮，§22 是第十九轮），**节号不重复、不跳号**。
> 这条不变量以前是破的——第三轮和第四轮都编成了 6，导致十一处「正文 §6」有一半指错地方
> （t45 重编号修掉，同轮还修了开头两处「见第 2 节的 A-1 / B-1」——它们其实在 §4）。
> 现在 `scripts/check-doc-links.py` 会把它连同全仓的锚点链接一起验，`sh scripts/check.sh` 和 CI 都跑。
> t46 起这条规矩不再只管这一份：README + `docs/` 下全部 15 份文档的 `§N`（共 166 处）都要指得到，
> 见文末[「§N 的规矩推广到全部文档了」](#n-的规矩推广到全部文档了)。

## 1. 规模与 panic 面

`src/` 23 个文件 8019 行，`tests/` 3941 行。

| 模块 | 行数 | `unwrap()` | `expect()` | 索引 | `as` 转换 |
|---|---:|---:|---:|---:|---:|
| cli | 2139 | 2 | 0 | 21 | 0 |
| runner | 911 | 0 | 3 | 1 | 5 |
| state | 561 | 0 | 1 | 0 | 1 |
| goals | 444 | 0 | 0 | 1 | 0 |
| log | 442 | 0 | 0 | 1 | 0 |
| tick | 390 | 0 | 0 | 7 | 3 |
| doctor | 347 | 0 | 0 | 1 | 0 |
| replan | 329 | 0 | 0 | 2 | 1 |
| hosts | 296 | 0 | **3** | 0 | 1 |
| 其余 14 个 | 1000 | 2 | 0 | 15 | 3 |

11 个 `unwrap`/`expect` 逐个看过，分三类：

**（一）真的会崩，且输入来自用户手改的文件 —— `hosts.rs:252/257/260`**

见 §4 的 A-1。这是这一轮唯一确认的 panic。

**（二）靠不变量保证，成立 —— `phase.rs:154`、`cli.rs:742`、`cli.rs:767`、`runner.rs:634`**

四处都是 `decide()` 返回 `should_run` 之后直接 `todo.unwrap()`。核过：
`tick.rs` 里 `should_run: true` **只有一处**构造，且同时 `todo: Some(...)`，所以成立。

但 `Decision` 是 `pub` 结构体、字段全 `pub`，这个不变量**没有任何东西守着**——
将来谁手工构造一个 `Decision { should_run: true, todo: None, .. }`，四处一起崩。
低危，记在 §4 的 B-1（**已在 t10 修掉**：构造器 + 私有字段 + `ready_todo()`）。

**（三）由构造保证，成立 —— `todo.rs:34`、`state.rs:275`、`runner.rs:328/329`**

`d.is_ascii_digit()` 之后才 `to_digit(10).unwrap()`；`and_hms_opt(0,0,0)` 恒有值；
`Stdio::piped()` 之后 `stdout.take()` 恒有值。

## 2. lint

```
cargo clippy --all-targets -- -W clippy::pedantic     → 全部风格类，无正确性问题
  96  #[must_use]          60  format! 追加到 String
  40  文档缺 # Errors      18  文档缺反引号
  ...
cargo clippy -W clippy::cast_possible_truncation -W clippy::cast_sign_loss
                                                      → 0 条（没有截断风险）
cargo clippy -W clippy::indexing_slicing              → 182 条（含测试），src 里 47 个点
```

**索引那 182 条在这个仓库里没有判别力**：抽查的每一个都是"索引来自一次已经校验过的
查找"（典型是 `todo::index_of()` 先返回 `Err` 才拿到 `idx`），clippy 看不出这层。
不建议打开这条 lint——它会淹掉真问题。

### 2.1 格式闸原先是空的（已修，t30）

`cargo fmt --check` 当时在**全仓 29 个文件**上都不合规、790 个 hunk。全红等于没有信号：
谁也不会去看一个永远失败的闸，格式漂移进来也没人拦。根因不是代码脏，是配置缺席——
这份代码是 ~125 列的密排风格，而仓库里没有 `rustfmt.toml`，rustfmt 就按默认的 `max_width=100` 判。

只调 `max_width` 还不够。rustfmt 另有一组"小启发式"（`fn_call_width=60`、`chain_width=60`、
`struct_lit_width=18`…），会把**没超 `max_width`** 的调用和结构体字面量也拆行。同为 `max_width=125`：

| 配置 | 一次性对齐的改动量 |
|---|---|
| 默认启发式 | 3200+ / 708− （多出 ~2400 行凭空的拆行） |
| `use_small_heuristics = "Max"` | 778+ / 406− |

结论：`rustfmt.toml` = `max_width = 125` + `use_small_heuristics = "Max"`，再跑一次 `cargo fmt` 对齐全仓。
之后 `cargo fmt --check` 退 0，格式闸第一次真的能用。
对齐那个提交只动格式，已进 `.git-blame-ignore-revs`（验证过：被拆行的 `tick.rs:160` 带上该文件后
blame 从 `ac040d3` 回到真正的作者提交 `dfe739c7`）。

**仍然没有 CI**：仓库里没有 `.github/`，所以这道闸目前只是"人可以跑的一条命令"，不是自动拦截。
真要拦，得再配一条 workflow 或本地 pre-commit——记在待办里，不在本条范围内。

### 2.2 闸有了定义，但没人自动去按（已修，t31）

t30 把格式闸从"永远红"修成"能过"，但它仍然只是**人可以跑的一条命令**。t31 补上自动那一半。

三件事：

**（一）先让 `clippy -D warnings` 真的能过。** 加闸之前先跑一遍，4 条红：
`awake.rs:9` / `notify.rs:8` / `notify.rs:9` 三条 `doc_lazy_continuation`（模块头注释里
一段列表后面直接跟正文，rustdoc 会把它并进最后一个列表项），`tick.rs:315` 一条
`needless_lifetimes`（`window_ticks<'a>` 的 `'a` 可以省）。
**这一步不能跳**：一道开局就红的闸和没有闸是同一件事，t30 已经在 `cargo fmt` 上踩过一次。

**（二）闸只写一份定义：`scripts/check.sh`。**
`fmt --check` → `clippy --all-targets --all-features -D warnings` → `cargo test`，按"越便宜越靠前"
排序、fail-fast。CI 调它，人也调它，两边永远是同一件事。带参数可只跑前几道
（`sh scripts/check.sh fmt clippy`）。

`cargo test` 这一道**故意不加 `--all-targets`**：加了会跳过 doc test，而这仓库的模块头注释是
主要的交接材料，doc test 是它们唯一的编译检查。

**（三）`.github/workflows/ci.yml` 跑在 macOS 上，不是 ubuntu。**
`awake::supported()` 是 `cfg!(target_os = "macos")`，非 macOS 上整个 keep-awake 层是 no-op，
而 7 个测试断言的正是 `pmset` 真的被调过。实测（把 `supported()` 临时改成 `false` 再 `cargo test`）：

```
failures:
    awake_reconcile_fixes_a_stale_setting
    hung_pmset_cannot_wedge_the_awake_probes
    runner_disables_lid_sleep_while_alive_and_restores_after
    sigterm_still_lets_the_runner_restore_the_sleep_default
    sleep_stays_disabled_until_the_task_finishes_by_itself
    watchdog_restores_default_after_kill_9_and_holders_are_reference_counted
    without_passwordless_sudo_runner_degrades_to_caffeinate_with_a_hint
test result: FAILED. 37 passed; 7 failed
```

在 ubuntu 上跑 = 开局 7 红，又回到 t30 那个坑里。

**闸真的会咬**（两次注入，跑完都还原了）：

| 注入 | 结果 |
|---|---|
| `src/notes.rs` 里塞一段缩进歪掉的函数 | 第一道 `fmt --check` 退 1，clippy / test **没有开跑**（fail-fast 生效） |
| 改成 `fn(v: &Vec<String>)`（格式干净） | 第一道过，第二道 clippy `ptr_arg` 退 101 |

**钉住"只有一份定义"**：`tests/gate_test.rs` 三条断言——CI 必须调 `scripts/check.sh`、
workflow 里不许内联那几条命令、`runs-on` 必须是 macOS。抄成两份之后它们会各走各的，
到那天"本地过了"和"CI 过了"就不是同一句话，而这种漂移平时看不出来。

**没做的两件（有意）**：

* **工具链没 pin**。跟着 runner 镜像的 stable 走，新版 clippy 加 lint 会让 CI 突然变红。
  代价是偶尔的意外红，换来的是不用维护一个越来越旧的版本号；真红了处置是"修掉或
  `allow` 掉"，不是把 `-D warnings` 摘了。当前绿在 rustc / clippy 1.98.0。
* **没把整道闸塞进 `policy.preflight_cmd`**。preflight 失败会记一笔 `fail` 而**不调宿主**，
  连红 `max_fail_streak` 轮 runner 就停机——把 `cargo test` 放进去等于让一棵红树把整个长跑
  卡死，runner 再也走不到"修好它"那一步。想要每轮开跑前过一道，用便宜的前两道——
  在 `.zloop/state.json` 的 `policy` 里写
  `"preflight_cmd": "sh scripts/check.sh fmt clippy"`（没有 `zloop policy` 这个命令，policy 是手改的）。

## 3. 测试覆盖空白

用"pub 函数名在 `tests/` 里一次都没出现"粗测（会低估：很多函数是通过 CLI 测试间接跑到的）：

| 模块 | pub fn | 测试没提到 | 其中值得注意的 |
|---|---:|---:|---|
| goals | 14 | 7 | `sanitize_id` `fresh_id` `resolve_match` |
| tick | 17 | 6 | `pending_feedback` `failures` `noop_streak` |
| log | 11 | 6 | `resolve_evidence` `read_section` |
| awake | 12 | 8 | 多数是平台相关，测不了 |
| session | 5 | 4 | `detect` `transcript_path` |
| **hosts** | 5 | **2** | **`install_claude_stop_hook`** ← 就是 A-1 那个（已修，换成报错） |
| phase | 3 | 3 | `compute` `reason_zh` |

**最值得记一笔的是最后一列那个巧合**：这一轮唯一确认的 panic 就在
`install_claude_stop_hook` 里，而它是 `hosts.rs` 五个 pub 函数中**没被任何测试提到**的两个之一。

## 4. 发现清单

严重度按「会不会让用户的东西坏掉 / 会不会让人看到错的结论」排，不按修起来难不难。

### A-1（高）`zloop install --claude-stop-hook` 在 settings.json 结构不对时 panic — 已修

`src/hosts.rs:252/257/260` 三处 `.expect()` 直接断言 JSON 的形状。
`~/.claude/settings.json` 是**用户和别的工具都会改**的文件，`"hooks": []` 这种写法完全可能出现。

实测（`HOME` 指向临时目录，喂进不同的 settings.json）：

| settings.json | 结果 |
|---|---|
| `{}` | ✅ wrote |
| `[]` | 💥 panic `hosts.rs:252` |
| `"hello"` / `42` | 💥 panic `hosts.rs:252` |
| `{"hooks": []}` | 💥 panic `hosts.rs:257` |
| `{"hooks": "none"}` | 💥 panic `hosts.rs:257` |
| `{"hooks": {"Stop": {}}}` | 💥 panic `hosts.rs:260` |
| `{"hooks": {"Stop": "off"}}` | 💥 panic `hosts.rs:260` |
| `{oops`（语法坏） | ✅ exit 2 + 说明 |

**语法坏的处理得很好，结构不对的直接崩。** 这条对比本身说明问题：作者想到了
"文件可能不是合法 JSON"，没想到"是合法 JSON 但不是我要的形状"。

修法：三处 `.expect()` 换成"结构不对就报错说清哪一层不对，并且别动这个文件"。
**尤其不能默默覆写**——那是用户的全局配置。

**已修**（`hosts.rs`）：三处 `.expect()` 换成 `ok_or_else(shape_err(...))`，报错点名是哪一层
（`顶层` / `hooks` / `hooks.Stop`）、放的是什么类型、要的是什么类型，并明说**没动这个文件**：

```
$ HOME=/tmp/x zloop install --claude-stop-hook      # settings.json = {"hooks": []}
zloop: /tmp/x/.claude/settings.json：hooks是数组，不是对象 {…}——没动这个文件。
       这是你的全局配置，请自己把hooks改成对象 {…}后重试，或者手动加一条
       command 为 `zloop hook-stop` 的 Stop hook
$ echo $?
2                                                   ← 和"文件不是合法 JSON"同一个出口
```

上表 7 行 `💥` 全部变成 `exit 2 + 说明`，且改完之后逐行比对过磁盘内容**一个字节没变**；
`{}` / `{"hooks":{}}` / `{"hooks":{"Stop":[]}}` 三种正常形状照旧写得进去。
回归测试 `a_wrongly_shaped_settings_json_is_reported_not_panicked_or_clobbered`
（`tests/cli_test.rs`）——三层分别换回 `.expect()` 都会让它 panic 变红。

### B-1（低）`Decision` 的 "should_run ⇒ todo 非空" 不变量没人守 — 已修（t10）

四处 `unwrap()` 依赖它，今天成立，但结构体字段全 `pub`，没有构造器也没有 `debug_assert`。
**不是 bug，是个绊子**——所以它从头到尾没有"修复前的失败场景"，只有"哪天有人碰了就会有"。
这一点决定了它该怎么修、也决定了它的回归测试长什么样（见下）。

**修法（三层，都在 `src/tick.rs`）**：

1. **构造器**：`Decision::ready(todo, interval_min)` / `stop(reason)` / `wait(reason, interval)`。
   `ready()` 把 todo 收成**参数**而不是 `Option` 字段——"说要跑却没说跑哪条"在这里写不出来。
   `decide()` 里原来的 3 处结构体字面量 + `hold_decision` 那 1 处全部改走构造器，
   现在全仓**一个 `Decision { … }` 字面量都不剩**。
2. **私有字段 `_seal: ()`**：字段照旧全 `pub` 可读（60 多处测试断言不用动），
   但别的模块拼不出 `Decision` 字面量。两个探针都真敲过（写进去看报错，再删掉）：

   ```
   # 同 crate 的模块（这才是判据——四处调用点就住在这里）
   error: cannot construct `Decision` with struct literal syntax due to private fields
      --> src/phase.rs:204:18
       = note: ...and other private field `_seal` that was not provided

   # 外部 crate（tests/）
   error: cannot construct `Decision` with struct literal syntax due to private fields
    --> tests/zz_seal_probe.rs:4:13
   ```

   也就是说"能造出违反不变量的 `Decision`"这件事从**全仓**收窄到了 `tick` 这一个模块，
   而那个模块里已经没有字面量了。
3. **读的那一侧**：`Decision::ready_todo() -> Option<&Todo>`（= `should_run` 且真有活）。
   四处调用点（`cli.rs:800` / `cli.rs:825` / `phase.rs:205` / `runner.rs:1097`）原来都是
   `if d.should_run { d.todo.as_ref().unwrap() }`，现在是 `if let Some(t) = d.ready_todo()`。
   runner 那处从 `.expect()` 换成 `let … else { return stop(root, &d.reason) }`：
   万一不变量真被破了，长跑该做的是像 `!should_run` 那样停下报原因，不是摔在那一行。

**回归测试** `tick_test::every_decide_exit_keeps_should_run_implies_a_todo`：
把 `decide()` 的 **13 个出口**（ready / paused / done / unplanned / all_done / all_deferred /
blocked / user_gate / blocked+exhausted / fail_streak / budget / progress_streak / throttled）
外加 `hold_decision` 各造一个 state 走一遍，逐个断言 `should_run == todo.is_some()`、
且 `ready_todo()` 和 `should_run` 同进同退；末尾再断言这 13 个 reason **真的都被走到了**
（fixture 防空跑，免得哪天 helper 一改全塌成 `ready` 还是绿的）。

撤掉修复变红（把 ready 那一支换回字面量 `should_run: true, todo: None`）：

```
thread 'every_decide_exit_keeps_should_run_implies_a_todo' panicked at tests/tick_test.rs:563:9:
assertion `left == right` failed: reason=ready 违反 should_run ⇒ todo 非空：
  Decision { should_run: true, reason: "ready", todo: None, interval_min: Some(3), _seal: () }
  left: true / right: false
```

**说清楚它守的是哪一半**：这条测试守的是 `tick` 模块**内部**——新加的出口只要违反不变量就红；
模块**外部**那一半由编译器守（上面那条 `error:`），不是由 `cargo test` 守。
两者都不是"复现一个今天存在的崩溃"，因为这条从来就没有。

**一处取舍（t31 刚把 clippy 变成闸，所以要写明）**：`clippy::manual_non_exhaustive` 会建议
把 `_seal` 换成 `#[non_exhaustive]`，这里**按下不表**（一条带理由的 `#[allow]`，只盖这一个
结构体）。理由是两者不等价：`#[non_exhaustive]` 只拦**别的 crate**，而这条不变量真正暴露
给的是 `cli` / `phase` / `runner`——它们和 `tick` 在**同一个 crate** 里，加了属性照样能拼出
字面量。私有字段拦的是**模块**，正好是需要的粒度。（`tests/` 是外部 crate，两种写法都拦得住，
所以上面那条探针不构成判据——判据是同 crate 的那三个模块。）

---

## 5. 第二轮：并发与持久化

### 做了什么

| 实验 | 做法 | 结果 |
|---|---|---|
| E1 并发写账本 | 20 个进程同时 `zloop plan --add` | ✅ 20/20 落地，id 不重复，JSON 可解析 |
| E2 并发写 NOTES | 20 个进程同时 `remember` / `remember --rule` | ❌ 见 A-2 |
| E3 写到一半被杀 | 400 次并发写 + 紧循环 `pkill -9`（386 个真被杀） | ✅ `state.json` 从没坏过；⚠ 见 A-3 |
| E4 锁残留 | 上面杀完之后接着敲命令 | ✅ 残留的 `.lock` 不挡人，`status` / `plan` 正常 |

`state.json` 这条路是**扎实的**：`transaction` = flock → load → mutate →
写 `.tmp` → `sync_all` → `rename`，386 次 SIGKILL 一次都没写坏。
flock 由内核在进程死亡时释放，所以不会留下卡死的锁。

### A-2（中）`zloop remember --rule` 并发会丢条目 — 已修（t6，commit d07d003）

`notes.rs` **一个锁都没有**，而 `add_rule` / `replace` 是**读-改-写**。实测：

| 场景 | 期望 | 实际 |
|---|---:|---:|
| 20 个并发 `remember`（纯追加，`O_APPEND`） | 20 | **20** ✅ |
| 20 个并发 `remember --rule`（读改写） | 20 | **12** ❌ |
| 10 追加 + 10 `--rule` 混合 | 20 | **16** ❌ |

追加是安全的，读-改-写不是。**混合场景更糟**：`--rule` 的整文件重写会把它读入之后、
写出之前那段时间里别人追加进来的经验一起吞掉。

不是理论问题：runner 起的 `claude -p` 子进程会敲 `zloop remember`，同一台机器上
另一个交互会话也会——这正是 `--rule` 那次重写窗口里最可能发生的事。

文件本身不会损坏（`write()` 也是 tmp + rename），丢的是**条目**——而且悄无声息。

修法：`notes` 的读-改-写走 `state::locked`（锁已经有了，`.zloop/state.json.lock`），
或者给 NOTES 自己一把锁。追加那条路可以不动。

### A-3（低）被杀之后留下 `state.json.tmp`，`doctor` 说"没发现问题" — 已修（t6，commit d07d003）

E3 之后 `.zloop/` 里躺着一个 3926 字节的半截 `state.json.tmp`。它**不影响正确性**
（下一次 `save` 用 `File::create` 覆盖它），但：

- 永远不会被清理，攒着占地方；
- **`zloop doctor` 对它完全沉默**——手工造一个 `.tmp` 再跑 `doctor`，输出是"没发现问题"。

`doctor` 的定位就是"只读体检 `.zloop`，逐条报问题和下一步该敲什么"，
残留的半截写入正是它该说一句的东西。

修法：`doctor` 加一条检查（发现 `*.tmp` 就报"上次写入被打断，可以删"）。

### 试过但没复现的

- **两个进程同时改同一份账本导致丢更新**：E1 20 并发，20/20 全落地、id 不重复。
  `transaction` 的 flock 挡住了。没复现。
- **杀进程导致 `state.json` 损坏或截断**：E3 386 次真 SIGKILL，每轮都验 JSON 可解析，
  一次都没坏。tmp + rename + `sync_all` 顶住了。没复现。
- **残留锁文件把后续命令锁死**：E4 杀完之后 `plan` / `status` 都正常。
  flock 是内核在进程死亡时释放的，`.lock` 文件留着不影响。没复现。
- **写回时日志文件和 tick 对不上**：`log::write` 在 `transaction` 的闭包**里面**，
  和 `save` 同属一把锁；就算在两者之间被杀，留下的是一个**孤儿日志文件**，
  而 `log::entries` 按 tick 账本过滤，孤儿不会被列出来。没有可见后果。

---

## 6. 第三轮：外部输入与进程边界

面：CLI 参数、stdin、环境变量、路径拼接、子进程调用、超时与信号。
做法是**逐项喂畸形输入并记下实际结果**，不是读代码猜。全程 132 个测试保持全绿，本轮没改一行代码。

### 6.1 逐项结果总表

| 输入面 | 试了什么 | 结果 |
|---|---|---|
| CLI 参数 · 空 | `init ""` / `init "   "` / `plan --add ""` | init 收下空目标（见 6.7）；plan 拒绝，说 "no todo lines found" |
| CLI 参数 · 超长 | 1 MB 的目标文字、1 MB 的 todo 文字 | ✅ 正常，输出自己截断 |
| CLI 参数 · 控制字符 | `\x1b[31m` ANSI、`\x07`、`\x08`、`\t` | ✅ 收下并原样存，没有转义注入 |
| CLI 参数 · 非 UTF-8 | `zloop remember $'\xff\xfe bad'` | ✅ clap 挡在门外：`invalid UTF-8 was detected` |
| CLI 参数 · 巨大数字 | `compact --keep-days` / `doc --since\|--until` | 💥 **A-8**：装得下 i64 就 panic（已修：exit 2 + 同一条友好提示） |
| CLI 参数 · 枚举越界 | `--priority 99` / `--status bogus` / `--keep-days -1` | ✅ clap 全部拒绝 |
| CLI 参数 · 未知 id | `edit nosuch` / `done nosuch` / `--blocked-by nosuchid` | ✅ 逐个报名字，exit 2 |
| CLI 参数 · 自依赖 | `edit t1 --blocked-by t1` | ⚠ **B-2**：收下，todo 永久卡死 |
| 路径 · 穿越 | `goal new --id ../../pwn` | ✅ `sanitize_id` 压成 `pwn`，父目录没被碰 |
| 路径 · 超长 | `goal new --id $(4096 个 a)` | ✅ 截到 40 字符 |
| 路径 · 全是废字符 | `goal new --id '!!!'` | ✅ 拒绝并说明只留 `a-z 0-9 . _ -` |
| 路径 · `--dir` 乱指 | 不存在 / `/dev/null` / `/` | ✅ 三条都是 "no zloop state at …"，exit 1 |
| 文件输入 · 不存在 | `plan --file /nope`、`done --evidence @/nope` | ✅ 带文件名报错，exit 2 |
| 文件输入 · 非 UTF-8 | `plan --file bad.bin`、`--evidence @bad.bin` | ✅ 报错，exit 2 |
| 文件输入 · 字符设备 | `plan --file /dev/zero` | ⚠ 无界读，挂住（见 6.6） |
| 文件输入 · 32 MB | `--evidence @big.txt` | ⚠ 不设上限（见 6.6），但账本和交接包没被污染 |
| stdin · 空 / 垃圾 / 深嵌套 | `hook-stop`、`replan --apply`、`reflect --apply` | ✅ 三个入口全部干净拒绝或静默忽略 |
| 环境变量 | `COLUMNS` = 0/1/2/-5/abc/超 u64、`NO_COLOR=`、`CLICOLOR_FORCE=1`、`ZLOOP_AWAKE_POLL_SECS=abc` | ✅ 9 种全部 exit 0，输出不变形 |
| state.json · 非 UTF-8 | 追加一个 `\xff` | ✅ 报错 exit 2，拒绝继续 |
| state.json · 数值越界 | `policy.window_hours` 手改大 | 💥 **A-7**：`next`/`status`/`context` 全 panic（已修：钳进 0..=8760 + doctor `bad_policy`） |
| NOTES.md · 非 UTF-8 | 追加一个 `\xff`；粘一行 GBK | 💥 **A-4**（已修）：静默清零 + 下一次写入真删 |
| 子进程 · 超时 | preflight / 宿主留下后台进程 | 💥 **A-6**（已修）：超时形同虚设，SIGTERM 也叫不动 |
| 信号 · SIGPIPE | `zloop status \| head` | ✅ `main.rs` 恢复了默认处置，安静退出 |
| 信号 · SIGTERM | `zloop stop` 打断正常轮次 | ✅ 干净退出并记 journal（已有测试） |

### A-4（高）NOTES.md 里一个非 UTF-8 字节 → 约定和经验静默清零，下一次 `remember --rule` 把它们真删掉 — 已修

`src/notes.rs:76`：

```rust
let Ok(raw) = fs::read_to_string(path(root)) else { return Notes::default() };
```

读不出来就**当成空文件**，不报错、不记一笔。而 `add_rule`（`notes.rs:187`）是读-改-写：
读到空的 `Notes`，加一条，然后 `write()` 整个覆盖回去。

实测（3 条经验 + 2 条约定的正常 NOTES.md，351 字节）：

| 步骤 | 文件里真有的 | `zloop context` 带的 | `zloop doctor` |
|---|---|---|---|
| 起点 | 2 约定 / 3 经验（351 B） | 2 约定 / 3 经验 | 没发现问题 |
| 追加一个 `\xff`（352 B） | 2 约定 / 3 经验**还在** | **0 / 0** | 没发现问题 |
| 再敲一次 `remember --rule` | **1 约定 / 0 经验**（201 B） | 1 / 0 | 没发现问题 |

第三步之后原来那 2 条约定和 3 条经验**从磁盘上消失了，没有备份**
（`ls .zloop/NOTES.md.bak-*` → 0 个）。CLI 当场还打印了
`约定 +1（共 1 条，每轮都带给模型）`——「共 1 条」就是唯一的破绽，而没人会盯着这个数。

不备份是**写在注释里的决定**（`notes.rs:181`）：

> **不备份**：加一条是纯增量，和 `reflect --apply` 的重写不是一回事

前提「加一条是纯增量」恰恰在 `read()` 降级成 default 的那一刻不成立。
`reflect --apply` 走 `replace()`，那条路是备份的；出事的是被判定为"安全"的那条。

**怎么会有非 UTF-8 字节**：NOTES.md 是设计上给人读、给人改的纯 Markdown。
实测最贴近现实的一条是**粘进一行 GBK 编码的中文**——这在一个全中文的项目里再普通不过：

```
$ python3 -c "open('.zloop/NOTES.md','ab').write('- 这条是 GBK 编码的\n'.encode('gbk'))"
$ zloop context | grep -c '粘进来的一条'
0            ← 上一条正常写进去的约定，没了
```

**同一个坏字节，两套政策**——这才是真正该改的地方：

| 文件 | 读到非 UTF-8 | 后果 |
|---|---|---|
| `.zloop/state.json` | `zloop: stream did not contain valid UTF-8`，exit 2 | 拒绝继续，数据安全 |
| `.zloop/NOTES.md` | 无声无息 | 交接包空了，下一次写入把原件删掉 |

修法三条，缺一不可（**三条都已修**）：
1. ✅ `notes::read` 区分「文件不存在」（返回 default 是对的）和「读失败」（要说话）：
   拆出 `try_read`（区分，给写路径）和 `read`（宽容，只给纯读路径）；
2. ✅ 读失败时 `add_rule` / `replace` **拒绝写**，不许在空 `Notes` 上重建文件（exit 2，原件一字不动）；
3. ✅ `doctor` 加一条 `unreadable_notes`：NOTES.md 存在但读不出来 → 报出来并说该怎么办。

第 3 条为什么不能省：1、2 只让**写**路径坏在明处，**读**路径照旧是静默降级——
`zloop context` 少掉「约定」「经验」两整节、退出码还是 0，模型当轮没有任何项目护栏而不自知，
而"下一轮一定会去写一次 NOTES"根本没人保证。体检就是替这条静默的读路径出声的地方
（`doctor_test.rs::unreadable_notes_reported_because_context_drops_the_rules_silently`
先复现那个静默，再钉住 doctor 必须报）。

### A-5（高）`--exit-on-wait` 在「等人」时从不生效——它只在一种 runner 自己走不到的状态下才管用 — 已修

`RunArgs` 说得很清楚：`Exit when waiting on a human instead of polling at the backoff ladder's last rung`
（当时写的是 `… at the slowest interval`，T34 把"最慢"改成了"末档"，承诺没变）。实测反过来。

```
$ zloop init "exit-on-wait probe"; zloop plan --add "[P0] needs a human"
$ zloop edit t1 --blocked-by user
$ zloop next --json --peek        → should_run=false reason=user_gate interval_min=10
$ zloop run --exit-on-wait --fast --max-rounds 1
runner: wait (user_gate) · sleeping 10 s
runner: wait (user_gate) · sleeping 10 s
runner: wait (user_gate) · sleeping 10 s
runner: wait (user_gate) · sleeping 10 s
>>> 30s 后还在跑
```

链条是三段，每一段单独看都对：

1. `tick::decide`（`tick.rs:233`）在 blocked / user_gate 分支给
   `interval_min: if exhausted { None } else { Some(...) }`，
   `exhausted = noop_streak >= max_noop_streak`；
2. `wait_plan`（`runner.rs:501`）先 `match d.interval_min`，
   **`Some(m)` 直接返回，根本不看 `opts.exit_on_wait`**；只有 `None` 那一支才问它；
3. runner 在 `!should_run` 时只写 journal 的 `sleep`、`continue`，**从不记 noop tick**
   ——实测空转 30 秒：journal 4 条 `sleep`，账本 `noop` **0 条**。

所以 `noop_streak` 恒为 0 → `exhausted` 恒为 false → `interval_min` 恒为 `Some`
→ `exit_on_wait` 这个字段在 runner 的等待路径上**是死代码**。
只有人在终端里手敲够 `max_noop_streak` 次 `zloop next` 才能把它唤醒。

**绿测试把它盖住了**，而且证据就写在测试自己的注释里
（`tests/runner_test.rs:182`）：

```rust
for _ in 0..3 {
    run(&d, &["next"], &[]); // exhaust noop streak so decide() says interval=None
}
// --exit-on-wait: old behaviour, exits at once
```

测试**先手工把状态搓成 `interval=None`**，再去验 `--exit-on-wait` 退出。
搓出来的这个状态正是真 runner 自己永远到不了的那一个。

**这条不是纸上谈兵**——审的时候这台机器上就抓到一个现行：

```
$ ps -o pid,lstart,etime,command -p 28061
28061  Sat Aug 29 11:44:02 2026   20:24:50
       zloop --dir /…/T/.tmpqSYkIf run --host claude --max-rounds 0
            --resume todo --timeout-min 30 --fast --exit-on-wait
$ tail -1 /…/.tmpqSYkIf/.zloop/runner/journal.jsonl
{"event":"sleep","until":"2026-08-30T08:09:14+08:00","reason":"user_gate",…}
```

一个带着 `--exit-on-wait` 的 runner，在 `user_gate` 上**转了 20 小时 24 分**，
写了 1849 条 journal（200 KB），并且一路占着 keep-awake（`zloop awake` 里那个 holder）。

修法（**已修**，`runner.rs::wait_plan`）：把 `exit_on_wait` 提到 `interval_min` 前面判——
「等人」这件事该不该退出，由标志决定，不该由 noop 计数决定。
顺带统一了继续等下去时的说法：只要是 `user_gate` / `blocked`，不管 `decide` 给的是哪一档
间隔（还是压根没给），journal 和终端上都写 `… (polling until a human unblocks)`；
原来这句话也锁在同一个到不了的 `None` 分支里，真 runner 上从来没打印过。

```rust
let human = d.reason == "user_gate" || d.reason == "blocked";
if human {
    if opts.exit_on_wait { return None; }              // ← 标志说了算
    let m = d.interval_min.unwrap_or_else(|| tick::ladder_tail(state));
    return Some((m, format!("{} (polling until a human unblocks)", d.reason)));
}
d.interval_min.map(|m| (m, d.reason.clone()))
```

复现与回归：

- `scripts/repro-a5-exit-on-wait.sh`——全程真实路径（init → plan → runner 起跑 → **宿主自己**
  `zloop done --block` 把 todo 交回给人 → 下一轮 runner 撞 user_gate）。
  修之前：15 秒后进程还在，journal 2 条 sleep、账本 **0 条 noop**，退出码 1；修之后退出码 0。
- `tests/runner_test.rs::exit_on_wait_stops_the_first_time_the_runner_itself_hits_a_human_gate`
  钉同一条路径，并顺带钉住「等人那一支一条 noop 都不记」这个前提（`--exit-on-wait`
  因此不能挂在 `noop_streak` 上）。
- 原来那两条测试（`waiting_on_a_human_polls_instead_of_exiting`、
  `wait_and_stop_trigger_notifications`）里的三次 `zloop next` 已删掉。
- 新增测试辅助 `run_within(…, limit)`：这一类「本该退出的 runner 不退出」的回归，用原来的
  `run()` 撤掉修复后是**挂住**而不是变红——挂住的测试没人会当成失败。撤掉修复实测：
  三条全部 FAILED（各 20–25 秒被上限掐掉），不是挂死。

**隔壁问题已查清**：`max_noop_streak` 在 runner 路径上不是「空转」，是**跨进程串味**——
见 [A-16](#a-16中高noop-计数从交互式命令串进-runner-的停机判断人敲三下-zloop-next就能让长跑拒绝启动--已修)。

### A-6（高）超时管不住留下后台进程的那一轮，而且这段时间里 SIGTERM 叫不动 runner — 已修

`run_with_timeout`（`runner.rs:325`）的 deadline 只守着 `try_wait` 那个循环。
跳出循环之后是：

```rust
stdout: h_out.join().unwrap_or_default(),
stderr: h_err.join().unwrap_or_default(),
```

两个线程在 `read_to_string` 上等管道 EOF。**直接子进程被 kill 不代表管道关了**——
孙进程继承了同一个写端，只要它还活着，EOF 就不会来，`join()` 就一直挂着。

实测（`--timeout-min 3 --fast` = 3 秒上限，每次都清空 `.zloop` 重来）：

| `policy.preflight_cmd` | 留下什么 | 超时 | runner 实际耗时 |
|---|---|---:|---:|
| `true` | 什么都没有 | 3 s | 1 s ✅ |
| `sleep 30` | 只有直接子进程 | 3 s | 3 s 后判超时 ✅ |
| `sleep 8 &` | 一个活 8 秒的孙进程 | 3 s | **10 s** ❌ |
| `sleep 20 &` | 一个活 20 秒的孙进程 | 3 s | **21 s** ❌ |

**耗时跟着孙进程的寿命走，跟 `--timeout-min` 没关系。**

宿主那条路一模一样。用一个「起了个后台服务然后正常返回」的假 `claude`：

```sh
#!/bin/sh
sleep 25 &                       # 这一轮 agent 用 Bash 起的后台服务
echo '{"session_id":"fake","is_error":false,"result":"起了个后台服务",…}'
```

```
[0s]  runner: round 1 → t1 [claude]
[26s] runner: round 1 NO WRITEBACK (recorded fail) · 起了个后台服务
```

宿主毫秒级就把 JSON 吐完了，这一轮却走了 26 秒——正好是那个后台进程的寿命。
把 25 秒换成一个真正的守护进程（`docker compose up -d`、dev server、`nohup … &`），
这一轮就**永远不结束**。

**放大一档：这段时间里 runner 叫不停。**
`stop_requested()` 只在 `try_wait` 循环里查，卡在 `join()` 上时谁也查不到：

```
[t=5s]  向 runner 发 SIGTERM（等价于 zloop stop）
[t=25s] 还活着 —— 只能 SIGKILL
```

而 SIGKILL 会跳过 `AwakeGuard` 的 `Drop`，keep-awake 不会被释放
（要靠 `zloop awake reconcile` 补）。

这一条直接顶在项目那句核心承诺上——「跑飞会停在人面前」。
卡死既不算跑飞、也不停、更不通知，`--timeout-min` 这道闸此刻是纸做的。

`preflight_cmd` 的文档举例就是 `./init.sh && cargo test`；一个 `init.sh` 起个后台服务
是最普通不过的写法。这不是构造出来的场景。

修法两条，**两条都已修**（`runner.rs::run_with_timeout`）：

1. **单开一个 process group**（`Command::process_group(0)`），超时/被叫停时
   `kill(-pid, SIGTERM)` → 0.5 秒宽限 → `kill(-pid, SIGKILL)` 收整组，孙进程一起走，EOF 自然来。
   代价：终端 Ctrl-C 不再直送子进程——runner 自己装了 SIGINT 处置，≤200ms 内替它收。
2. **排水有上限**（`DRAIN_GRACE = 2s`）。直接子进程一收掉，它自己的输出就**已经全在管道缓冲里**，
   2 秒只是读出来的时间；等不到 EOF 就把线程扔下不 `join`（`join` 就是重新挂死），
   已经读到的半截照常返回，并往 stderr 说一句"这一轮的输出是截断的"。
   这一层是给**孙进程 `setsid` 逃出了进程组**准备的兜底——第 1 条这时候够不着它。

两条各管一半：孙进程还在组里时靠 1，逃出去了靠 2。少任何一条，
"永远不结束"都还在（`sh -c 'cmd &'` 这种 sh 秒退的写法根本走不到超时分支，只有 2 拦得住）。

**复现**（`runner_test::timeout_collects_the_background_grandchildren_too` /
`::sigterm_reaches_the_runner_while_a_grandchild_holds_the_pipe`）：
`preflight_cmd = "sh -c 'sleep 4; : > MARK' & sleep 8"`，闸设 2 秒。撤掉修复：

```
超时那一轮走了 8.160998167s，`--timeout-min 2` 没兜住
SIGTERM 之后 6 秒 runner 还活着：它卡在排水上，只能 SIGKILL（A-6）
```

实景（`--timeout-min 3 --fast`，孙进程活 25 秒、前台再挂 30 秒）：

| 场景 | 修前 | 修后 |
|---|---:|---:|
| 3 秒的闸到点 | 96.3 s | 15.6 s（3 轮 fail_streak，每轮 ~3.5 s） |
| 等超时期间发 SIGTERM（t=5s） | t=30.2 s 才退 | **t=5.4 s** |
| 孙进程 | 等它自己咽气 | 当场收掉 |

### A-7（中）`policy.window_hours` 手滑一下，`next` / `status` / `context` 全 panic，而 `doctor` 说没问题 — 已修

`tick.rs:186` `at - Duration::hours(state.policy.window_hours)`，中间没有任何范围检查。

`.zloop/state.json` 不是内部文件——zloop **自己就在教人去手改它**
（`cli.rs:623`：「改大 `.zloop/state.json` 里的 `policy.max_total_usd`，再 start」）。
既然人被引导着去编辑那个 `policy` 块，隔壁字段被顺手改错就只是时间问题。

| `window_hours` | 结果 |
|---|---|
| `24`（默认） | ✅ |
| `0` | ✅ |
| `99999999999` | 💥 panic `tick.rs:186` `DateTime - TimeDelta overflowed` |
| `-99999999999` | 💥 同上 |
| `999999999999999999` | 💥 panic chrono 内部 `TimeDelta::hours out of bounds` |

炸的是哪几条命令（`window_hours = 99999999999`）：

| 命令 | 结果 |
|---|---|
| `zloop next` | 💥 panic（exit 101） |
| `zloop status` | 💥 panic |
| `zloop context` | 💥 panic |
| `zloop doctor` | ✅ exit 0 —— **"没发现问题"** |
| `stats` / `log` / `replan` / `compact` | ✅ 正常 |

**炸掉的正好是每轮都要走的那三条**（skill 每轮 `context` → `next`，runner 每轮 `decide`），
而唯一一个专门用来回答"哪儿不对"的命令一声不吭。整个项目目录就此敲不动，
人拿到的是一行 Rust panic 加一句 "run with RUST_BACKTRACE=1"。

修法：`window_hours` 读进来时钳到合理范围（或用 `checked_sub_signed`，越界就退回默认值并说一句），
外加 `doctor` 补一条 policy 数值体检。这和 §5 的 A-3、和上面的 A-4 是同一个毛病的第三次出现：
**`doctor` 只查它想到的那几样，没有一条兜底的「我自己走一遍主路径看看炸不炸」。**

**已修**（`tick.rs` + `doctor.rs`），两半：

1. **算的地方钳住**。新增 `tick::WINDOW_HOURS_MAX = 24 * 365` 和
   `tick::window_span(policy)`，取值一律先 `clamp(0, WINDOW_HOURS_MAX)` 再交给 chrono；
   `window_ticks` 和 throttle 那一支的 `frees_in` 都改走它，外加 `checked_sub_signed` /
   `checked_add_signed` 兜底（`at` 和 `oldest` 都可能是从账本里读来的，一个都不信）。
   上表五个取值（含 `i64::MAX` / `i64::MIN`）现在 `next` / `status` / `context` 全部 exit 0。
2. **doctor 说出来**。新增 `bad_policy` 检查——钳过就等于**人写的那个数没生效**，
   静悄悄按别的数跑比崩掉更难查：

```
$ zloop doctor          # state.json 里 policy.window_hours = 99999999999
✗ [repro] policy.window_hours = 99999999999，不在 0..=8760 里
   ——配额窗口按 8760 小时算，你写的那个数没生效
   → 把 .zloop/state.json 的 policy.window_hours 改回 24（默认）或别的 0..=8760 的数
```

同一条检查顺手覆盖另外两个"写了不生效"的取值：`max_total_usd` 为负（错误级，
花费只增不减 = 这个目标一轮都跑不了）、`intervals_min` 为空（警告级，有 3 分钟的兜底）。

回归测试：`an_out_of_range_window_hours_gets_clamped_instead_of_panicking` /
`in_range_window_hours_is_left_alone`（`tests/tick_test.rs`，钳位本身 + 合法取值不被误伤）、
`an_out_of_range_window_hours_does_not_take_the_whole_project_down`（`tests/cli_test.rs`，
人真敲的那三条命令）、`policy_numbers_written_out_of_range_are_reported`
（`tests/doctor_test.rs`）。撤掉钳位 → 前两个 panic 变红、CLI 那个 `left: 101 / right: 0`；
撤掉 `check_policy` 调用 → 后两个变红。

#### A-7 复核（t28）：两处钳位分属两条分支，CLI 面只盖住了一条

A-7 的验收只点名了 `tick.rs:186` / `state.rs:270` / `cli.rs:1990`，而越界的 `window_hours`
实际有**两处**要钳，走的是两条不同的分支：

| 位置 | 什么时候走到 | 撤掉钳位的症状 |
|---|---|---|
| `tick.rs:290` `window_span` | 每次 `decide` 都走 | `TimeDelta::hours out of bounds`（exit 101） |
| `tick.rs:377` 等待封顶 `window_hours * 60` | **只有配额占满**（`counted >= max_runs`）才走 | `attempt to multiply with overflow`（exit 101） |

复核结论：两处**都已经钳住**，`i64::MAX` / `i64::MIN` 在内的 8 个取值 ×
14 条命令（含 `throttled` 分支）全部不崩。但复核时发现**测试覆盖有个洞**：
CLI 那条测试的 fixture 从不写 tick，`max_runs` 永远没满，于是**它根本走不到第二处**——
实测撤掉 `tick.rs:377` 的钳位，旧版 CLI 测试照样是绿的（只有 `tick_test` 那条抓得到）。
已把 fixture 补成 `quota_full` ∈ {false, true} 两种，并加一条防空跑的断言
（`quota_full && hours > 0` 时 `reason` 必须真的是 `throttled`）；现在撤掉任一处钳位，
CLI 这条测试都会变红。

**这个洞的通用形状**：一处防御写在只有特定状态才走到的分支里时，
"测试绿了"说明不了它被验过——先确认 fixture 真的把那条分支走到了（断言它的可见结果），
再谈这条测试盖住了什么。

### A-8（中）时间参数「装得下 i64」就 panic，装不下反而有好错误提示 — 已修

两个入口，同一个根因（`state.rs:270` 和 `cli.rs:1990` 都是 `now() - Duration::…(n)`，无 checked）：

| 命令 | 结果 |
|---|---|
| `compact --keep-days 99999999999` | 💥 panic `cli.rs:1990` |
| `compact --keep-days 999999999999999` | 💥 panic chrono `TimeDelta::days out of bounds` |
| `compact --keep-days 9223372036854775807` | 💥 同上 |
| `doc --since 99999999999d` | 💥 panic `state.rs:270` |
| `doc --until 99999999999h` | 💥 panic `state.rs:270` |
| `doc --since 99999999999999999999d` | ✅ **exit 2 + 「看不懂的时间…用 2h / 30m / 7d」** |
| `doc --since ""` / `"  "` | ✅ 同上 |
| `compact --keep-days -1` | ✅ clap 拒绝 |

最后那几行才是这条的价值：**大到 i64 装不下的反而被接得漂漂亮亮**
（`digits.parse::<i64>()` 失败 → 落到友好错误那一支），
刚好装得下的直接崩。作者想到了"这串东西可能不是数字"，没想到"是数字但算不出来"。

和 §4 的 A-1 是同一个思维缺口：**校验了形状，没校验取值范围。**

修法：`parse_when` 和 `cmd_compact` 都换成 `checked_sub_signed`，
越界就走已经写好的那条友好错误路径。

**已修**（`state.rs` + `cli.rs`）：

- `parse_when`：`Duration::minutes/hours/days` → `try_minutes/try_hours/try_days`，
  再 `now().checked_sub_signed(...)`。算不出来时**不 return**，直接掉进下面那条
  已经写好的路径——于是"是数字但算不出来"和"根本不是时间"给的是同一句话：
  `doc: --since 看不懂的时间 "99999999999d"：用 2h / 30m / 7d、2026-08-29，或完整的 ISO 时间戳`（exit 2）。
- `cmd_compact`：同样换 `try_days` + `checked_sub_signed`，越界时
  `compact: --keep-days 99999999999 太大了，算不出截止时间；用天数，比如 30`（exit 2）。
  **验参挪到 `ensure_idle` 之前**：一个连范围都不对的参数，不该先去抢那道闸。

上表 5 个 `💥` 全变成 exit 2；`--since 7d` / `--keep-days 30` 这些正常取值一条没被误伤。
回归测试 `out_of_range_time_arguments_get_the_same_friendly_error_as_garbage`
（`tests/cli_test.rs`，两个入口 × 越界取值 + 正常取值），
两个入口分别撤回原实现都会让它 `left: 101 / right: 2` 变红。

### B-2（低）`edit <id> --blocked-by <它自己>` 被收下，那条 todo 就再也跑不了 — 已修（t12，commit ba87ca2）

```
$ zloop edit t1 --blocked-by t1
t1 [P1] open …                     ← exit 0，收下了
$ zloop next
WAIT (blocked) remaining 1 · retry in 10 min      ← 永远
$ zloop doctor
没发现问题
```

`edit` 校验了「blocked_by 里的 id 存不存在」（`--blocked-by nosuchid` 会被拒），
但没校验「是不是它自己」。唯一的 todo 自锁之后目标就停在那儿，
配合 A-5 就是一个不会退出、也不会有人被通知的 runner。

多条 todo 组成的环（t1←t2←t1）属于调度逻辑，留给 t4 那一轮系统地过。
这里只记 `edit` 这个**入口**该挡而没挡。

**修复记录（t10 复核时补）**：这条**在 t12 修 A-9 时就被顺手做掉了**，只是当时没回来
把状态写在这儿——t10 按待办去修，先跑了一遍才发现已经是绿的：

- 闸在 `cli.rs::cmd_edit`（`deps.contains(&me)` → exit 2，错误话术里连"为什么永远满足不了"
  一起说），事务里判、判完直接返回，**一个字都不写进 `state.json`**；
- 回归测试 `cli_test::edit_refuses_to_make_a_todo_depend_on_itself`：拒了 + 说了为什么 +
  `blocked_by` 空着 + `next --peek` 照旧 `ready`，最后再验"挡的是依赖自己、不是 `--blocked-by`
  本身"（`edit t1 --blocked-by t2` 照收）；
- **入口是完整的**：全仓写用户给的 `blocked_by` 只有 `cli.rs:1093` 这一处
  （`grep -n "blocked_by\s*[=:]" src/`，另一处 `tick.rs:547` 是 `done --block` 追加 `user` 标记），
  所以挡住 `edit` 就挡住了全部命令行入口；手改文件造出来的那种由 `doctor` 报
  （`doctor_test::a_self_dependency_from_a_hand_edited_file_is_reported_but_a_finished_dep_is_not`）。

所以 t10 在这条上**没有改代码**，只把状态补正。

### 6.6 记一笔，但不算 bug 的

- **`--evidence @<大文件>` 不设上限**：32 MB 的证据文件，日志文件涨到 32 MB，
  峰值内存 132 MB。但**账本和交接包都没被污染**（`state.json` 1079 字节、
  `tick.note` 1 字符、`zloop context` 540 字节）——该有的边界都在，
  只是落盘那一份没截。对比 `log::changed_files` 是明确截过的。
- **`plan --file /dev/zero` / `--evidence @/dev/zero` 会挂住**：`read_to_string` 无界读。
  要拿字符设备喂它才触发，场景太构造，记一句就够。
- **`zloop log --show ../../../../etc/hosts` 能读任意文件**：`cmd_log` 先试 `PathBuf::from(name)`
  再回退到 `.zloop/log/`。这是个本地 CLI，用户对这些文件本来就有读权限，
  **不构成越权**，而且 `--show` 的帮助文字写的就是「path or bare file name」。不算发现。

### 6.7 试过但没复现的

- **路径穿越写到 `.zloop/` 外面**：`goal new --id ../../pwn` 被 `sanitize_id` 压成 `pwn`
  （`/` 不在保留集里 → 变 `-`，开头的 `.` `-` 再被 `trim_matches` 削掉）。
  4096 字符的 id 截到 40。父目录下没出现任何新文件。没复现。
- **环境变量把输出搞崩或算错宽度**：`COLUMNS` 喂 0 / 1 / 2 / -5 / abc / 20 位数字，
  加上 `NO_COLOR=`、`CLICOLOR_FORCE=1`、`ZLOOP_AWAKE_POLL_SECS=abc`，
  9 种全部 exit 0 且输出行数不变。`style.rs` 一路 `.ok()` + `saturating_sub`，扛得住。没复现。
- **stdin 喂垃圾把命令搞崩**：`hook-stop` 喂空 / 乱文本 / `[]` / 2000 层嵌套数组，
  全部 exit 0 静默忽略（钩子就该这样）；`reflect --apply` 空输入明确拒绝并说怎么办。没复现。
- **超长 / 控制字符的 CLI 参数**：1 MB 的目标文字、1 MB 的 todo、
  带 `\x1b[31m` 和 `\x07` 的目标——都正常收下、正常显示、正常存 JSON。没复现。
- **非 UTF-8 从命令行钻进来**：clap 直接挡（`invalid UTF-8 was detected in one or more arguments`）。
  文件那侧（`plan --file` / `--evidence @`）也都是明确报错。**唯一漏的是 NOTES.md**，见 A-4。
- **`cargo test` 漏进程**：机器上确实躺着一个 2026-08-29 11:44 起、活了 20 小时的 runner，
  但今天完整跑了两遍 `cargo test`（132 测试全绿），临时进程都正常收掉了，**没能复现漏的那一下**。
  那个残留进程为什么至今不死，是 A-5 而不是漏进程——它自己带着 `--exit-on-wait`。

---

## 7. 第四轮：调度逻辑的边界

全部用真命令构造，`zloop next --json` 读结果。

| 场景 | 实际行为 | 判断 |
|---|---|---|
| 0 条 todo（刚 init） | `unplanned` + "还没有待办 · 先用 zloop plan" | ✅ |
| 全部 deferred | `done` + "0 条待办全部完成，目标结束（另有 2 条延后）" | ❌ B-3（已修） |
| 自依赖 `t1←t1` | `blocked`，"30 分钟后重试"，doctor 沉默 | ❌ A-9（已修） |
| 二元环 `t1←t2←t1` | 同上 | ❌ A-9（已修） |
| tick 时间戳在 2099 | `ready`（不撞配额时无影响） | ✅ |
| tick 时间戳在 1970 | `ready` | ✅ |
| tick 时间戳是乱码 | `ready`，不崩 | ✅ |
| **tick 在未来 + 撞配额** | `throttled`，`interval_min=38048610`（72 年） | ❌ **A-11**（已修） |
| `max_runs = -5` | 反序列化就拒绝："expected usize"，exit 1 | ✅ |
| 五个阈值分别设 0 | 三个当"关闭"，两个当"永远触发" | ❌ A-10（已修） |

### A-11（高）时钟跳到未来 + 撞配额 = runner 睡 72 年，而 status 看着一切正常 — 已修

`tick.rs` 的 throttle 分支拿"窗口内最老那条 tick"算还要等多久：

```rust
let frees_in = oldest + Duration::hours(policy.window_hours) - at;
let minutes = (frees_in.num_seconds().div_euclid(60) + 1).max(1) as u32;
```

`oldest` 要是在未来，`minutes` 就是个天文数字。**没有上限**，
`runner.rs` 的 `secs(units, fast) = units * 60` 也不封顶。实测：

```
$ zloop start
runner started in the background (pid 88578, host claude)

journal:  {"event":"sleep","until":"2099-01-02T00:00:09+08:00","reason":"throttled"}
$ zloop status
  阶段    两轮之间的休息 · 睡到 00:00 醒，还有 38048608m55s
```

**两个问题叠在一起才致命**：

1. 等待时间没有上限——一次时钟跳变就能让 runner 睡到下个世纪；
2. `status` 只印 `00:00`**不印日期**，"睡到 00:00 醒"和正常的轮次间隔长得一模一样。
   那串 `38048608m55s` 是唯一的线索，而它长得像个 ID。

触发条件不需要有人手改文件：NTP 校时、改时区、虚拟机挂起恢复、笔记本电池耗尽后时钟重置，
都会让已有的 tick 落在"未来"。**这正是"跑了一夜回来发现没进展"的一种，而且状态面板还告诉你一切正常。**

**修法**：三处一起，缺任何一处这一夜还是白跑的。

(a) **等待封顶在配额窗口本身**（`tick.rs` 的 throttle 分支）——一条 tick 最多在窗口里
待 `window_hours`，等得比这更久没有任何道理：

```rust
let cap = policy.window_hours.clamp(1, 24 * 365) * 60;
let minutes = (frees_in.num_seconds().div_euclid(60) + 1).clamp(1, cap) as u32;
```

`window_hours` 先 `clamp` 再乘，是因为它来自手写得了的 `state.json`：不夹住的话
`window_hours * 60` 自己会溢出，封顶反而成了新的越界口子。正常（过去的）窗口不受影响，
照旧精确算到分钟——回归测试里把 `22*60+1` 那条一起钉住了。

(b) **`status` / `context` 的"睡到"跨天带日期**（`phase.rs` 的 `hhmm` → `when(ts, now)`）：
同一天只印 `HH:MM`，跨天印 `%m-%d %H:%M`，跨年印 `%Y-%m-%d %H:%M`。比日期前先把两边
统一到看的人所在的时区（偏移不同的话同一瞬间会落在不同的"今天"）。
同一个函数也用在"第几轮开始于"和"在飞的活领于"两处——同一类信息，同一个口径。

(c) **`doctor` 报 `future_timestamp`**：封顶只是不让它睡死，那条 tick 还占着配额位——
`window_ticks` 收的是「时间戳 ≥ now − window_hours」，未来的 tick **永远**满足，
`max_runs` 一满窗口就再也滑不开。所以还得有人来看一眼。光这几条未来 tick 就把
`max_runs` 占满时报 Error（循环已经限流住了），否则报 Warn；5 分钟以内的偏差算正常的
机器间时钟漂移，不报。

回归测试两条：`a_future_tick_cannot_stretch_the_throttle_wait_past_the_window`（单元，
撤掉封顶 → `Some(38054161)` vs `Some(1440)`）和 `a_clock_jump_into_the_future_is_capped_and_visible`
（端到端，真命令走 `done` → 改钟 → `next --peek --json` → `status` → `doctor --json`，
三处分别撤掉都能变红）。

### A-9（中高）依赖成环没人拦，永久卡死且无诊断 — 已修

`zloop edit t1 --blocked-by t1` 直接被接受。此后：

```
zloop next   → should_run=false  reason=blocked  interval=10
zloop status → 阶段  等依赖 · 30 分钟后重试
zloop doctor → 没发现问题
```

**"30 分钟后重试"会一直重试下去**——依赖永远不会满足。二元环 `t1←t2`、`t2←t1` 一模一样。

`doctor` 已经有一条 `dangling_blocked_by`（依赖指向不存在的 todo），但**环不在它的检查范围里**。

修法：`edit --blocked-by` 拒绝自依赖；`doctor` 加一条环检测（不必挡住 edit 的多跳环，
但至少要报出来）。

**已修**，两处：

(a) **`edit --blocked-by` 挡住自依赖**（`cli.rs`，退 2 并说明为什么）。只挡这一种，
是因为它是**唯一一个不看全图就能判定**的环——多跳环要不要算，还取决于依赖做没做完
（`t2 ← t1` 而 t1 已 done 是正常形状，不是环），把那层判断塞进 `edit` 会在写到一半的
依赖图上误伤。多跳环交给体检。

(b) **`doctor` 新增 `dep_cycle`**（`doctor.rs::check_dep_cycles`）：三色 DFS + 显式栈
（todos 是从文件里读来的，链有多长不由我们说了算，不能递归），撞到回边就把当前路径上
那一圈原样印出来——`依赖成环：t1 → t2 → t1（→ 读作「依赖」）`，人一眼知道断哪条。
两个边界值得记：

- **边只在「依赖还没 done」时才算数**。`is_executable` 认的就是 `status == done`，
  依赖做完的那条线已经不挡任何人。不加这条，每个正常用了 `--blocked-by` 的项目都会
  被报出一堆解释不清的假环。
- **环上全是了结掉的 todo（deferred / cancelled）报 Warn 不报 Error**：这会儿卡不住谁，
  但捡回来就会。少了这一档，`doctor` 会在一个跑得好好的项目上退 1。

回归测试三条（`doctor_test.rs`）：二元环用**真命令**造（`edit` 只挡自依赖，这条路仍然
走得通，正是它值得体检的理由）并先钉住 `next → blocked` 这个前提、再断开一条确认
doctor 立刻闭嘴；自依赖从手改的文件进来；"依赖已 done" 的正常形状一个字都不报。
加 `edit` 那条（`cli_test.rs`）共四条，撤掉任一处修复都变红。

### A-10（中）"0 = 关掉这个检查"只对三个阈值成立 — 已修

```
max_runs > 0 &&              ← 0 = 关
max_total_usd > 0.0 &&       ← 0 = 关
max_progress_streak > 0 &&   ← 0 = 关
fail_streak(ticks) >= policy.max_fail_streak      ← 没有 > 0 守卫
noops >= policy.max_noop_streak                   ← 没有 > 0 守卫
```

实测：全新项目、**一次失败都没有**，`max_fail_streak: 0` →
`should_run=false, reason=fail_streak, interval=None`。目标当场永久卡死。

`max_noop_streak: 0` 稍隐蔽：它不改 `should_run`，但让 `exhausted` 恒真，
于是被依赖挡住时 `interval_min` 从 30 变成 `None`（"停下等人"）而不是继续轮询。

想关掉某个检查的人，按另外三个的先例写 0，得到的是相反的效果。

**已修**：`tick::decide` 的两处各补一个 `> 0` 守卫，五个阈值口径统一成「0 = 关掉」。
注意 `fail_streak()` 内部的 `forgive_at > 0` 早就有了（`feedback` / `edit` 的清零条件），
少的一直是**外面那道判定**——所以修复前的表现不是"数不准"，是一次失败都没有就停机。
`max_noop_streak = 0` 更隐蔽：`should_run` 不变，只有 `interval_min` 从 `Some(10)`
塌成 `None`，面板上看着像"正常地在等人"，而无头 runner 就此不再自己醒来。

关掉之后不需要 `doctor` 再说什么——0 现在是一个**说得通的设置**，报它就是噪音。
回归测试一条（`tick_test.rs::zero_turns_a_threshold_off_the_same_way_for_all_five`）：
两个字段各验"写 0 = 真关掉（连着 5 次 fail 也不停）"和"写正数照旧管用"，
另外三个原本就对的一起钉住，免得哪天被改歪；两处守卫分别撤掉都变红。

### B-3（重估为中）全部 deferred 时说"目标结束"，并引着人去开新目标 — 已修

第四轮把它记成"措辞不准"的低危。**回来修的时候发现比记的严重**：它根本走不到 `all_done`
那一支，因为在那之前 `goal.status` 就已经被改成 `done` 了。

`cli.rs` 的 `edit` 收尾（原 1026–1031）：

```rust
let open = !todo::open_ordered(st).is_empty();
if st.goal.status == "done" && open { st.goal.status = "active".into(); }
else if !open { st.goal.status = "done".into(); }   // ← 延后最后一条 = 目标结束
```

`is_terminal` 含 `deferred`，所以把最后一条待办延后就清空了 open 列表，目标当场被标成
**已结束**。修复前实测（真命令，`init` → `plan` 两条 → `edit --status deferred` 两条）：

```
$ zloop next --json
  "reason": "done",  "should_run": false,  "interval_min": null
$ python3 -c "…" .zloop/state.json → goal.status = done

$ zloop status
  ✅ 完成      ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  清单    0/0 完成 · 2 条延后
  阶段    0 条待办全部完成，目标结束（另有 2 条延后）
  加活    zloop plan --add "[P0] 下一件事"
  换目标  zloop goal new "新目标"

$ zloop start
start: 没启动——runner 起来第一轮就会退出（done）。
原因：当前目标已经结束了。
下一步：zloop goal new "<新目标>"
```

一条活都没做，三个出口（status 的页脚、start 的下一步、skill 决策树的"已完成 → goal new"）
**全部指向"丢掉这个目标"**。skill 模板里那句"只有 `all_done` 才是真做完了"也拦不住它——
它报的连 `all_done` 都不是，是 `done`。两条被推到以后的活就此没人再看。

**修法**（和 `unplanned` 当初从 `all_done` 里分出来同一个套路）：

1. `todo::all_deferred(state)` = 清单非空 且 每条都是 `deferred`（open 空时"没有一条 done"
   就等价于"全是 deferred"，因为非终态的都在 open 里）。
2. `cli.rs` 的 `edit` 和 `tick::apply_done` 的收尾共用同一口径：`finished = open 空 && !all_deferred`，
   只有 `finished` 才标 `goal.status = done`。`edit` 那边反过来也成立——发现不是 finished
   就把误标的 `done` 改回 `active`，所以**修复前存量的坏状态会在下一次 edit 时自愈**。
3. `tick::decide` 的空清单分支从两路变三路：`unplanned` / `all_deferred` / `all_done`。
4. 出口全部改指向"把活捡回来"：`status` 的标题词（`⏭ 全部延后`）、阶段句、页脚
   `捡回来 zloop edit t1 --status open`（带上第一条延后 todo 的真 id）；`start_refusal`
   的 `all_deferred` 一支；`context` 的「下一条」和「待办」两节；`phase::reason_zh`；
   skill 模板里补第三个词。

修完实测同一条路径：

```
$ zloop next --json → "reason": "all_deferred"      goal.status = active
$ zloop status
  ⏭  全部延后  ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  阶段    2 条待办全被延后了，一条都没完成 · 目标没结束，只是没活可跑
  捡回来  zloop edit t1 --status open
$ zloop start
原因：2 条待办全被延后了，没有能跑的。
下一步：zloop edit <id> --status open 把要做的那条捡回来，或 zloop plan --add "[P0] 下一件事"
$ zloop edit t1 --status open && zloop next --json → ready / t1
```

回归测试两条，撤掉修复都变红：

- `tick_test::all_deferred_is_not_all_done` — 撤掉 `decide` 的三路分支：
  `left: (false, "all_done")` / `right: (false, "all_deferred")`
- `cli_test::all_deferred_is_not_the_goal_finishing`（走真命令，不手搓状态）— 撤掉 `edit`
  的 `!all_deferred` 守卫：`延后最后一条不该把目标标成 done  left: "done"  right: "active"`

后一条同时钉住了**不该动的那一边**：1 条 done + 1 条 deferred 仍然是"目标结束"。

### 试过但没复现的

- **坏时间戳把调度器搞崩**：tick 的 `at` 换成 `not-a-time`，`decide` 照常返回 `ready`，
  不崩也不误判（`parse_iso` 失败的那条被跳过）。
- **负数 policy 造成下溢**：`max_runs: -5` 在反序列化阶段就被拒（"expected usize"），
  exit 1 且信息清楚，压根进不了调度器。
- **0 条 todo 被当成"全部完成"**：报的是 `unplanned`，措辞也对。这条以前是 bug，已经修过了。

---

## 8. 第五轮：实景撞上的

不是扫出来的——是这次长跑自己踩的。审查会话和无头 runner 共用一个工作树，
`--git-commit` 的 checkpoint 当场把对方没写完的代码提交了。

### A-12（高）`--git-commit` 的 checkpoint 提交整个工作树 — 已修

`runner.rs` 原来的 `git_checkpoint`：

```rust
git(&["add", "-A", "--", "."])?;
let _ = git(&["reset", "-q", "--", ".zloop"]);
git(&["commit", "-q", "-m", &format!("zloop {todo_id}: {note}")])?;
```

`add -A -- .` 是**整棵树**。runner 并不知道哪些改动是这一轮的 agent 干的，于是
工作树里任何别人的在制品——另一个会话改了一半的文件、还没 `add` 的实验、
甚至编译不过的半截函数——都会被卷进一条消息写着"zloop t16: <我这一轮干了什么>"的提交。
提交消息说的是这条 todo，内容却是两个人的。runner 只打印一行 `git checkpoint <sha>`，
不说它到底装了什么。

还有一条更隐蔽的：`git commit` 会把**索引里已经暂存的一切**带走。别人 `git add` 完
还没 commit 的东西，即使这一轮谁都没碰过它，也会被 checkpoint 顺手提交掉。

**复现**（`runner_test::git_checkpoint_leaves_foreign_work_in_progress_out`）：起跑前在树里
留三样别人的东西——未跟踪的 `broken.rs`（编译不过）、已跟踪 `shared.rs` 的半截改动、
`git add` 过的 `staged.txt`——然后让 runner 带 `--git-commit` 跑两轮。撤掉修复：

```
assertion `left == right` failed
  left: ["broken.rs", "shared.rs", "staged.txt", "t1.txt"]
 right: ["t1.txt"]
```

三样全在"zloop t1: wrote t1.txt"这条提交里。

**修法**：runner 起跑时给工作树拍一张快照（路径 → 大小:mtime，`.zloop/` 除外），
checkpoint 只提交这条线之后变化的路径，且用 `--pathspec-from-file` 显式点名
（顺带解决索引里别人暂存物的问题，也不受 argv 长度限制）。三类路径三种处理：

| 快照里的状态 | 处理 |
| --- | --- |
| 快照里没有 → 我们在场时冒出来的 | 提交 |
| 在快照里且没变 → 别人的在制品，没人动过 | 留着不管 |
| 在快照里但变了 → 两个人的改动缠在一个文件里 | **不提交**，打印+记账本 `commit_held_back` |

第三类拆不开，所以宁可不提交也不能把别人的半成品塞进我的提交——但必须**出声**，
这正是原来最糟的地方：它一声不吭。

快照**只在 commit 成功后**刷新，不是每轮刷新。差别在于那些没写回的轮次：
agent 干了活但 host 没写回 → 不 checkpoint → 改动留在树里。如果按轮刷新快照，
下一轮就会把它当成"这轮开始前就脏的"外人东西永远扔掉。
`runner_test::git_checkpoint_reclaims_work_left_by_a_round_that_never_wrote_back`
钉住这一条（它在旧代码上也是绿的——它防的是修复本身跑偏，不是复现 bug）。

**已知边界**（A-12 修完时留下的，见下条 A-13）：runner 跑着的时候别人**新建**的文件仍然
分不出来（快照里没有 = 算我们的）。要根治得每轮重新拍快照，而那会牺牲上面"认领上一轮
遗留"的能力。

### A-13（高）快照只拍一次，长跑中途冒出来的都算我们的 — 已修

A-12 的规则"不在基线里 ⇒ 是我们的"只在基线足够新时成立，而基线是**起跑那一刻**拍的，
之后只在 commit 成功时刷新。于是这条线管的不是"这一轮"，而是"上次成功提交以来的一切"——
中间的每一段睡眠、每一轮回看、每一轮重估、每一轮没提交成的活，全算在内。
`interval_min` 3–30 分钟、一轮活几分钟，长跑里**大部分墙上时间根本不在轮次里**，
邻居在这些时间里新建的任何文件都会被下一轮的 checkpoint 认领走。

比 A-12 描述的更宽：不只是"新建"。任何在基线那一刻是**干净**的路径，之后被别人改脏，
都会落进"快照里没有"那一格。

**复现**（`runner_test::git_checkpoint_leaves_out_a_file_a_neighbour_creates_between_rounds`）：
两轮的 runner，两轮之间睡 3 秒；邻居等 runner 在账本上写下 `sleep` 事件（这时第一轮的
checkpoint 早跑完了）再新建一个 `neighbour.rs`。撤掉修复：

```
assertion `left == right` failed: neighbour.rs
t2.txt
  left: ["neighbour.rs", "t2.txt"]
 right: ["t2.txt"]
runner: git checkpoint 35eea29 · 2 个文件
```

`neighbour.rs` 进了"zloop t2: wrote t2.txt"。写这个用例时踩过一次：一开始拿"commit 出现在
`git log` 里"当邻居的落笔信号，结果测不出 bug——commit 落地到 `*baseline = git_dirty()`
之间只有几毫秒，邻居抢进那道缝里写，文件反而被算进了新基线。信号要挑**基线刷新之后**
才出现的，`journal.jsonl` 里的 `sleep` 事件正好是。

**修法**：每轮开工前（`should_run` 之后、回看/重估/宿主之前）重拍一次基线——上一轮收尾到
这一刻，我们一个宿主都没在跑，这时候新冒出来的脏东西一件都不是我们的。

**但只在"上一轮结清了"时重拍**，这就是 A-12 留下的那个取舍的答案：`Checkpoint::settled`
为真表示树里已经没有我们欠着没提交的东西（提交成功了，或者压根没有我们的东西要提交）。
没写回的轮次、`git add`/`commit` 失败的轮次都不结清，基线原地不动，产物留给下一轮认领。
`settled` 在放宿主进来之前立刻清掉，所以"上一轮没收干净"这件事不会跨轮漏判。

方向上宁可多认不可漏认：多认了提错文件，还能从 git 历史里挑出来；漏认了那轮的活永远
提交不了，而且没有任何一处会再提起它。

另一个更彻底的方案（每轮无条件重拍 + 单独维护一份"我们欠着的路径"跟着往下传）能把
没写回那一轮之后的窗口也补上，但要在 4 条 `continue` 分支上都记对账，记错一处就是永久丢活；
换来的只是"失败轮之后那一轮"这一格。不值，没做。

**真正剩下的边界**：邻居在**我们的宿主正在跑的那几分钟里**新建的文件，仍然算我们的。
两边在同一个窗口里写，工作树不记录作者，任何快照差分都判不了——除非改成从宿主的
工具调用流里读它到底写了哪些文件（`--output-format stream-json`），那是另一件事。
所以 checkpoint 现在**把提交了哪几个文件打出来**（不只是几个），账本 `commit` 事件也带
`paths`：判不出来的那一格，至少让人事后能认出来。

---

## 9. 第六轮：A-6 的同一类死法，另外三条路

A-6 修的是宿主/preflight 那条路（`run_with_timeout`：进程组 + 排水上限）。
这一轮沿着同一个问题问下去：**runner 每轮还起了哪些子进程，它们有闸吗？**

### 9.1 全部子进程调用点

| 位置 | 命令 | 什么时候跑 | 有闸吗 |
|---|---|---|---|
| `runner.rs:198` | `sh -c <preflight_cmd>` | 每轮 | ✅ `run_with_timeout`（A-6） |
| `runner.rs:600/643` | `claude` / `codex` | 每轮 | ✅ `run_with_timeout`（A-6） |
| `runner.rs:223` | `git status --porcelain -z -uall` | **每轮开工前**（`--git-commit`） | ❌ 裸 `.output()` |
| `runner.rs:292` | `git rev-parse` ×2 | 写回后 | ❌ 裸 `.output()` |
| `runner.rs:296/323` | 同上 `git status` 再两次 | 写回后 | ❌ 裸 `.output()` |
| `runner.rs:344` | `git add` / `git commit` | 写回后 | ❌ 裸 `wait_with_output()` |
| `notify.rs:37` | `curl` | 通知时 | ✅ `-m 10` |
| `notify.rs:75` | `sh -c <notify_cmd>` | 通知时（wait/stop/限流/超预算） | ❌ 裸 `wait_with_output()` |
| `log.rs:56` | `git diff` / `git ls-files` | `zloop done` 里 | ⛔ 在宿主进程树里，被宿主那道闸兜住 |
| `awake.rs:44/55` | `sudo -n pmset` | 起跑/收尾各一次 | `-n` 不会等输入，不会挂 |

一轮里有 **6 次没有闸的 git 调用**（只在 `--git-commit` 下；这次长跑正是这么跑的），
外加通知那条路上一条用户自己配的 `sh -c`。

### A-14（高）git 一挂住，runner 跟着挂住，而且 SIGTERM 叫不动它 — 已修

裸 `.output()` / `wait_with_output()` 是**无限期**的阻塞等待：既不看 `--timeout-min`，
也不看 `stop_requested()`。和 A-6 一模一样的死法，只是从排水那根管子换到了 git 上。

**「索引锁争用会挂住」这条不成立**，先排掉（git 2.50.1 实测）：

| 场景 | `git status` | `git add` | `git commit` |
|---|---|---|---|
| `.git/index.lock` 被别人占着 | 0.0s rc=0 | **0.0s rc=128** | **0.0s rc=128** |

锁争用是**秒失败**，不是挂住——而失败这条路 zloop 是处理对的：`git_pathspec` 把 git 的
stderr 打出来，`Checkpoint::settled` 保持 false，基线不重拍，这一轮的产物留给下一轮认领。

**真正挂得住的是钩子和文件系统**（同一套实测）：

| 场景 | 命令 | 耗时 |
|---|---|---|
| `pre-commit` 钩子 `sleep 5` | `git commit … --pathspec-from-file=-` | **5.9s** |
| `core.fsmonitor` 钩子 `sleep 5` | `git status --porcelain -z -uall` | **5.4s** |

两条都不是构造出来的：`pre-commit` 是 husky / lefthook / pre-commit 框架的默认落点，
里面跑测试、跑 lint、等一把网络锁都很常见；`core.fsmonitor` 和网络文件系统（NFS、
sshfs、被挂起的容器卷）stall 是同一格——git 卡在读工作树上。

**runner 层面的复现**（`scripts/repro-a14-git-hang.sh`，三种模式都退 1）：

```
$ sh scripts/repro-a14-git-hang.sh commit
[setup] pre-commit 挂 987s → 卡在 git_checkpoint 的 commit 上
[t=12s] runner 还在跑（这一轮的闸是 5 秒），发 SIGTERM —— 等价于 zloop stop / 关机
RESULT: ❌ SIGTERM 之后 10s，runner (pid 17572) 还活着 —— 只剩 SIGKILL

$ sh scripts/repro-a14-git-hang.sh status
[setup] core.fsmonitor 挂 987s → 卡在开工前的 git_dirty 上
RESULT: ❌ SIGTERM 之后 10s，runner (pid 18030) 还活着 —— 只剩 SIGKILL

$ sh scripts/repro-a14-git-hang.sh notify
[setup] notify_cmd = sleep 987s → 卡在收尾那一下的通知上（活全干完了却退不出去）
RESULT: ❌ SIGTERM 之后 10s，runner (pid 18881) 还活着 —— 只剩 SIGKILL
```

耗时跟着 git 走，跟 `--timeout-min` 没关系（`--timeout-min 5 --fast` = 5 秒的闸）：

| `pre-commit` 挂多久 | runner 整轮耗时 |
|---|---:|
| 0s | 2s |
| 5s | 8s |
| 15s | 17s |
| 987s | 一直挂着，SIGTERM 无效，只能 SIGKILL |

**三条路各有各的难看法：**

1. **`commit`（写回后）**：活干完了、账本记了 `end`，就差最后那一下提交。人从外面看是
   「这一轮做完了」，实际上 runner 再也不动了。
2. **`status`（开工前）**：卡在宿主起跑之前。`run.log` 只有一行 keep-awake，
   `journal.jsonl` 只有 `awake_on`——**账本上一个字都没有**，看不出它在哪儿卡着。
3. **`notify`（收尾）**：全部 todo 都完成了，`stop()` 在发通知时挂住——
   于是不记 `stop`、不清 pid 文件、退不出去。「干完就停」这句承诺卡在最后一米。

**余波：SIGKILL 之后仓库是坏的。** 人被迫 `kill -9`（`zloop stop` 5 秒后就是这么干的），
但 runner 没给 git 单开进程组、也不收尸，那个 `git commit` 变成孤儿活着，
`.git/index.lock` 一直攥在它手里：

```
余波: .git/index.lock 还在 —— 这个仓库后面所有 git 写操作都会失败：
  fatal: Unable to create '/tmp/zloop-a14-commit.QVqjjt/.git/index.lock': File exists.
```

这时候**人自己的 `git add` / `git commit` 也一起废了**，直到有人手动删锁。

信号选择上有个关键差别（用 runner 那条 `--pathspec-from-file` 的命令形实测）：

| 给挂住的 `git commit` 发 | `.git/index.lock` |
|---|---|
| SIGTERM | **git 自己清掉了** ✅ |
| SIGKILL | 留在原地 ❌ |

（注意命令形要对：不带 pathspec 的 `git commit` 在跑 `pre-commit` 时**还没**拿锁，
拿 pathspec 的那种是先建索引再跑钩子，所以才踩得到。测错命令形会得出"没有锁"的假结论。）

所以修法必须是 **SIGTERM 优先**——正好是 `stop_group()` 已经在做的事。

**观测面**：卡住的那 14 秒里 `zloop status` 只报 pid 活着（「后台 runner 在跑（pid …）」），
`zloop doctor` 说「没发现问题」。谁都没法把"卡死的 runner"和"正在干活的 runner"分开。

**修法（留给单独一条 todo）**，三步：

1. 把 git 调用换成一个带闸的 `run_capture(cmd, timeout, stdin) -> Captured`，
   复用 `run_with_timeout` 已有的三件套：`process_group(0)` + `stop_group()`（TERM→0.5s→KILL）
   + 排水上限。**不能直接复用 `run_with_timeout` 本身**，两处不兼容：
   - 它把 stdout 转成 `String`（`from_utf8_lossy`）。`git_dirty` 要的是**字节**：`-z` 的
     NUL 分隔 + 路径可能不是 UTF-8。lossy 会把一个叫不出名字的路径变成"叫得出但是错的"，
     然后 `git add <错路径>` 会让**整个 checkpoint 失败**（现在的代码专门注释了这一点）。
   - `git_pathspec` 要往 stdin 喂 pathspec，而 `run_with_timeout` 写死了 `stdin(null)`。
2. 闸给多少：git status/add/commit 在正常仓库是亚秒级，大仓库 `status` 可能十几秒。
   建议固定一个宽松值（60s 量级）而不是复用 `--timeout-min`（那是给宿主的，动辄几十分钟，
   等于没闸）。超时按"这一轮 checkpoint 失败"处理：`settled` 保持 false、打印一行、
   记一条账本事件，产物留给下一轮认领——这条路现有代码已经通了。
3. 超时收掉 git 之后**检查 `.git/index.lock` 还在不在**，还在就明确说出来
   （别自动删：可能是别人正在用的锁，删了会毁掉对方的操作）。

顺带把 `notify.rs` 那条 `sh -c <notify_cmd>` 也套上同一个闸（同类，同修法）。

**修法已落地**（t19）。三条全做了，另加一条修的时候才发现的：

| | 落点 |
|---|---|
| 1. 带闸的 `run_capture(cmd, timeout, stdin_bytes) -> CapturedBytes` | `runner.rs`。`process_group(0)` + `stop_group()`（TERM→0.5s→KILL）+ `DRAIN_GRACE` 三件套只此一份；`run_with_timeout`（宿主）退化成它的文本版包装。**stdout/stderr 是字节**，要文本的自己转 |
| 2. 闸给 60 秒 | `ZLOOP_GIT_TIMEOUT_SECS` 可调（通知那条是 `ZLOOP_NOTIFY_TIMEOUT_SECS`，30 秒）。不复用 `--timeout-min`——那是给宿主的，动辄几十分钟 |
| 3. `.git/index.lock` 只报不删 | `index_lock_left()`：收掉 git 之后看一眼，还在就打一行「后面所有 git 写操作都会失败，确认没有别的 git 在跑之后手动删」 |
| 4.（新）`git_dirty` 的返回值改成 `Option` | 原来读不出工作树就退回**空快照**，而空快照 = 「树里所有脏东西都是我们的」。闸装上之后这条路会被真正走到（git 被收掉 = 读不出来），不改就成了新的丢工作方式：checkpoint 会把邻居的在制品一起提交 |

`git_capture()` 把「起不来 / 超时 / 被叫停 / 非零退出」合成一种失败，因为调用方对它们的
处置完全一样：**这一轮不提交**，`Checkpoint::settled` 保持 false，基线不重拍，产物躺在
树里等下一轮认领。超时和被叫停会额外记一条 `git_stalled` 账本（`cmd` / `how` /
`index_lock_left`），因为那两种事从外面看不出来。

另外两处 `stop_requested()` 检查是新加的（开工前拍基线之后、checkpoint 之后）：
SIGTERM 落在 git 子进程身上时，只有这两个位置看得见「有人叫停了」——不加的话
runner 还会白起一轮宿主才发现。

三种模式的复现脚本现在全退 0，SIGTERM 之后 1 秒内退出：

```
$ for m in commit status notify; do sh scripts/repro-a14-git-hang.sh $m; done
[setup] pre-commit 挂 987s → 卡在 git_checkpoint 的 commit 上
RESULT: ✅ SIGTERM 之后 1s 内退出
  | runner: git commit 被叫停，已经整组收掉（60s 的闸）
[setup] core.fsmonitor 挂 987s → 卡在开工前的 git_dirty 上
RESULT: ✅ SIGTERM 之后 1s 内退出
  | runner: git status 被叫停，已经整组收掉（60s 的闸）
  | runner: 开工前读不出工作树，沿用上一张基线（checkpoint 会更保守）
[setup] notify_cmd = sleep 987s → 卡在收尾那一下的通知上
RESULT: ✅ SIGTERM 之后 1s 内退出
  | notify: command 被叫停，已经整组收掉（通知发没发出去不知道）
  | runner: stop (done)
```

回归测试四条（`tests/runner_test.rs`），撤掉修复三条变红、第四条靠一次定点变异验红：

* `hung_git_commit_is_cut_off_and_the_work_is_reclaimed_next_round` —— 超时那一轮不提交、
  记一条 `git_stalled{how:timeout}`，第一轮的产物跟着第二轮一起进历史（证明 `settled` 是 false）
* `hung_git_status_does_not_wedge_the_round` —— 开工前读不出工作树时**沿用上一张基线**
  （断言那句话在 stderr 上），这一轮照常写回
* `hung_notify_cmd_does_not_wedge_the_stop` —— 收尾通知挂住不拖垮 `stop()`
* `a_path_git_cannot_name_back_does_not_sink_the_whole_checkpoint` —— 守住「字节，不是 String」：
  把 `git_dirty` 的 stdout 过一遍 `from_utf8_lossy`，这条就红成
  `fatal: pathspec 'bad<?>.txt' did not match any files`——**整轮 checkpoint 陪葬**。
  路径是用 `git update-index --cacheinfo` 塞进索引的：APFS 自己拒绝非 UTF-8 文件名（Errno 92），
  但 git 存字节，索引里放得下。

测试自己也带闸（`run_bounded`）：要证明的就是「不会挂住」，挂住时 `cargo test` 该当场
变红说清楚，而不是安静地卡到有人来按 Ctrl-C。

**同类但没修**：`log.rs::changed_files()` 里还有三次裸 git（`zloop done` 写回时跑的）。
它跑在**宿主进程**里而不是 runner 里，挂住了会被 `--timeout-min` 那道闸兜住，
所以不是同一个严重度——单开一条记着（下面 A-15）。

### A-15（中高）写回路上的裸 git 挂住 → 这一轮的账和技术文档一个字都没落盘 — 已修

排 t20 时本来只想「评估要不要一并走 `run_capture`」。评估的结论是**要**，而且它比原先估的
一档还高一点：不是「日志少一节」，是**整轮白干**。

**为什么比想的严重**：`cli.rs:887` 那行 `changed_files(root)` 跑在 `state::transaction`
**之前**。它挂住的时候，note / approach / decision / pitfall / evidence 全都还在内存里，
一个字节都没进 `state.json`。挂多久，这一轮的产物就在磁盘上不存在多久。
（对比 A-14：runner 那边挂住时，宿主干的活至少还躺在工作树里，下一轮能认领回来。）

**复现**（`git diff --stat HEAD` 卡在 `core.fsmonitor` 上，钩子 `sleep 991`）：

```
$ zloop done t1 --note n --approach a      # 修复前
STILL RUNNING after 20s (pid 6021)
 6021     1 zloop done t1 --note n --approach a
 6040  6021 git diff --stat HEAD -- .       ← 挂在这儿
ticks: 0                                    ← 写回一个字都没落盘
```

**「有 `--timeout-min` 兜着」这句话在两个方向上都站不住**：

1. 它兜的是**尺寸不对的**闸。默认 30 分钟（`cli.rs:321`），一次列改动文件的 git 挂住
   要烧掉整整 30 分钟，烧完那一轮记成 `fail`，写回的内容照样没了。
2. 交互路径**根本没有这道闸**。人在终端里跑 `/zloop`、或者自己敲 `zloop done`，
   没有任何东西会来收它。

**修法**：三次 git 走 `runner::run_capture`（A-14 那份带闸的实现），共用**一个**总预算
`git_timeout()`（`ZLOOP_GIT_TIMEOUT_SECS`，默认 60s）——调用方等的是「列一下改了什么」
这一件事，不是三件，没道理给 3×60 秒的最坏情况。超时就少写那一节，并且**在 stderr 上说一句**
（少了的原因写不进日志本身——日志正是写不下去的那个东西，同 t15 的 NOTES.md）。

**这里有个和 A-14 反向的取舍：进程组。** `run_capture` 原来一律 `process_group(0)` 单开一组，
好让超时/叫停时 `killpg` 把孙进程一起收掉。**照搬到 `zloop done` 上是错的**：它是短命 CLI，
在 runner 场景里跑在宿主进程里，宿主超时是整组 `killpg` 收掉的——单开一组的 git 会从那一刀
底下逃走，没人再管它的闸（管闸的父进程已经死了），于是永远挂着。挂着的 git 可能正拿着
`.git/index.lock`，那正是 A-14 里最不能留的东西。

实测两种选择（`zloop done` 自己当组长，模拟上层对宿主整组下刀）：

```
Group::Own      → zloop pid 8966, git child 8987 → RESULT: git child SURVIVED as orphan ❌
Group::Inherit  → zloop pid 8813, git child 8834 → RESULT: git child died with its caller ✅
```

所以 `run_capture` 多了一个显式的 `Group` 参数，两条路各自说清楚自己漏掉谁：
`Own` 收得掉孙进程、收不掉自己变孤儿；`Inherit` 不留孤儿、收不掉孙进程（钩子的 `sleep`
会漏出来，但它不拿锁，比孤儿 git 便宜）。runner 侧全部 `Own`，写回侧 `Inherit`。
`stop_group` 跟着分叉：`Inherit` 时信号发给直接子进程，**不拿子进程的 pid 当组 id 去赌**。

**修好之后**（同一个复现，闸压到 5s）：

```
exit=0 elapsed=7s                            # 5s 闸 + 2s 排水
done: 读工作树的 git 超过 5s 没回来，已经收掉；这一轮的日志不带「改动文件」清单
ticks: 1   note: n                           ← 写回完成，技术文档在
no index.lock ／ git add OK                  ← 仓库没被留下的锁废掉
```

回归测试两条（`tests/runner_test.rs`），各自撤掉对应的修复就变红：

* `hung_git_in_write_back_does_not_swallow_the_round` —— 闸装没装。撤回裸 `.output()`：
  `zloop done ... 过了 20s 还没退出 —— 子进程没有闸（A-14 的死法）`。
  **上限压在钩子那 30 秒之下**是这条测试成立的前提：`hook_that_hangs_once` 只挂得住 30 秒，
  一开始写的 60 秒上限让裸 `.output()` 也「通过」了（30.1s 跑完，绿的）——差点收下一条假绿。
* `write_back_git_dies_with_its_caller` —— 组选对没有。`Inherit` 改成 `Own`：
  `git 11563 从调用者的组里逃走了，变成没人管的孤儿（Group::Own 的死法）`。

149 测试全过（原 147），clippy 无新增告警。

**还剩的裸子进程**：`awake.rs` 里的 `pmset` / `sudo` / `caffeinate` / `visudo`（8 处）。
它们不在每轮的热路径上，`pmset` 和 `visudo` 也不是会挂住的那一类，但「每一个子进程都走
`run_capture`」这句话目前还不是真的——单开一条记着。

## 10. 第七轮：noop 计数的作用域

### A-16（中高）noop 计数从交互式命令串进 runner 的停机判断：人敲三下 `zloop next`，就能让长跑拒绝启动 — 已修

A-5 收尾时留了个问号：`max_noop_streak` 在 runner 路径上是不是死策略？
查下来结论比「死」难看——它**不是不生效，而是从错误的进程生效**。

三段事实，逐条都验过：

1. **`noop` tick 只有一个生产者**：`zloop next` 在 `should_run=false` 且非 `--peek` 时记
   （`cli.rs:763`）。全仓 `tick::record` 的调用点一共 8 处，runner 那 4 处只记
   `reflect` / `fail` / `replan`，**一条 noop 都不记**（实测跑一轮再空转：账本 0 条 noop）。
2. **但账本是共用的**：runner 每轮开头 `state::load` → `tick::decide`，读的就是 `zloop next`
   写进去的那些 tick。`decide` 里 `exhausted = noop_streak >= max_noop_streak`。
3. **`wait_plan` 把 `None` 一律当「停」**。A-5 已经把 `user_gate` / `blocked` 摘出来了，
   于是 `exhausted` 只剩最后一个出口：`throttled`（`tick.rs:267`）。

合起来就是：人在终端里敲三下 `zloop next` 想看一眼现在什么情况，
把一个本该「睡到配额窗口放开再接着跑」的 runner 变成了「拒绝启动」。

同一份状态，唯一的差别是有没有人敲过 `zloop next`
（`scripts/repro-a16-noop-poke-kills-throttled-runner.sh`，修之前）：

```
=== 场景 A：没人碰过它 ===
  runner: 睡着（还活着） ｜ 账本里 runner 记的 noop：0 条
  journal: {"event":"sleep","until":"2026-08-30T12:08:02+08:00","reason":"throttled"}

=== 场景 B：人敲了 3 下 zloop next ===
  $ zloop next → WAIT (throttled) remaining 1 · retry in 1440 min   （×3）
  账本里现在有 3 条 noop（全是 zloop next 记的）
  runner: 退出了：runner: stop (throttled)
  $ zloop start → start: 没启动——runner 起来第一轮就会退出（throttled）
```

**为什么修法是「让 runner 别读」而不是「让 runner 也记」**：
配额窗口是**自己会滑过去**的，`decide` 连还差几分钟都算出来了（上面那条 `retry in 1440 min`），
等下去一定等得到。让 runner 也记 noop，等于让每一次撞配额的长跑在第三次退避时自杀——
方向正好反了。这和 A-5 是同一条原则：**该不该退出由 runner 自己的规矩定，不由计数定。**

修法（`runner.rs::wait_plan`）：

```rust
if d.reason == "throttled" {
    // 等的是时间，不是人，所以 `--exit-on-wait` 不管这一支。
    let m = d.interval_min.unwrap_or_else(|| tick::ladder_tail(state));
    return Some((m, format!("{} (sleeping until the quota window frees)", d.reason)));
}
```

修完之后 runner 的规矩收敛成两条，`max_noop_streak` 对它**再无任何影响**：

| | reason | runner 怎么办 |
|---|---|---|
| 等得到的 | `user_gate` / `blocked` | 睡下去再看（`--exit-on-wait` 可改成退出） |
| | `throttled` | 睡下去再看（等的是时间不是人，那个标志不管它） |
| 等不到的 | `paused` `done` `unplanned` `all_done` `all_deferred` `fail_streak` `progress_streak` `budget` | 停 |

顺带删掉 `cli.rs::start_refusal` 里的 `"throttled"` 分支：`start` 的拒绝理由全部来自
`immediate_stop_reason`，而它现在再也不会返回 `throttled`，那条文案已成死代码。

README 里 `max_noop_streak` 那一行原来写的是「（runner 不受此影响）」——**话是对的，代码不是**。
现在两边对齐了，并在「`next` 怎么决定」下补了一节讲清这条边界。

复现与回归：

- `scripts/repro-a16-noop-poke-kills-throttled-runner.sh`：A/B 两个一模一样的项目，
  只有 B 被人敲过 3 下 `zloop next`。修之前退出码 1（B 秒退），修之后 0（两边都睡着）。
- `tests/runner_test.rs::interactive_next_pokes_cannot_kill_a_throttled_runner`：
  全程真实路径（真跑一轮填满 `max_runs=1` → 敲三下 `next` → `zloop start`）。
  撤掉修复实测变红：
  `配额窗口会自己滑过去，start 不该拒绝：start: 没启动——runner 起来第一轮就会退出（throttled）`。
  测试里先钉住「`next --peek --json` 确实给出 `reason=throttled, interval_min=null`」这个前提，
  否则 `max_noop_streak` 默认值一改，这条测试就悄悄变成在测空气。

151 测试全过（原 150）。

## 11. 第八轮：把「交互式命令写进账本的东西」系统扫一遍

A-16 只是一个样本。这一轮把问题反过来问：**runner 的判断一共读账本里的哪些量，
这些量分别有谁在写？** 只要写的人里有一个是交互式命令，那就是一条 A-16 同型的路。

### 11.1 runner 读什么（判断输入的全集）

`tick::decide` + `runner::wait_plan` + `runner::run` 里，所有影响「跑不跑、跑哪条、
停不停、resume 谁」的量，一个不落：

| # | 输入 | 从哪算出来 | 影响什么 |
|---|---|---|---|
| 1 | `goal.status` | 直接读 | `paused` / `done` → 停 |
| 2 | `todos[].status` | `open_ordered` / `all_deferred` | `unplanned` / `all_done` / `all_deferred` → 停 |
| 3 | `todos[].blocked_by` | `todo::executable` | `user_gate` / `blocked` → 睡 |
| 4 | `ticks[].outcome`（尾部连续） | `fail_streak` | `fail_streak` → **停** |
| 5 | 同上 | `progress_streak` | `progress_streak` → **停** |
| 6 | 同上 | `noop_streak` | A-16 修完后 runner **不再读**，只剩 `zloop next` 的退避提示 |
| 7 | `ticks[].cost_usd` 求和 | `spent_usd` | `budget` → **停** |
| 8 | `ticks[].at` + `COUNTED` | `window_ticks` | `throttled` → 睡 |
| 9 | ~~**`ticks.len()` 的轮内增量**~~ → 轮内有没有 `done/progress/fail/block` | `runner.rs:1069` 的 `wrote_back` | 记不记 `fail`（→ 4）、要不要 checkpoint 提交（A-17 修完后不再看长度） |
| 10 | `ticks[].host` / `.session` | `pick_session` | 这一轮 `--resume` 谁 |
| 11 | `in_progress` | `held_by_other` | 只挡交互式 `next`，**对 runner 无效**（有意，见 `tick.rs:108`） |
| 12 | `policy.*` | 直接读 | 上面所有阈值 |

### 11.2 谁在写（写命令的全集）

`state.json` 的全部写入点（`state::transaction` / `state::save` 的调用者）逐个对照：

| 命令 | 往账本里写什么 | 碰到上面哪几项 | 结论 |
|---|---|---|---|
| `init` / `plan` | todos、`goal.status` | 1 2 | 有意 |
| `next`（非 peek） | `noop` tick、`in_progress` | 6 9 11 | 6 已在 A-16 修掉；9 已在 A-17 修掉（原来只躲过 `noop` 一种） |
| `done` | done/progress/fail/block tick | 2 4 5 7 8 9 | 有意——它本来就是写回 |
| `edit` | **`edit` tick**、todo 状态/依赖、`goal.status` | 1 2 3 4 5 **9 10** | 打断 streak 是有意的；9 已在 A-17 修掉；**10 未修** → A-19 |
| `feedback` | **`feedback` tick** | 4 5 **9 10** | 9 与 4 已在 A-17 修掉；**5（`progress_streak`）和 10 未修** → A-19 |
| `replan --apply` | `replan` tick、换 todo | 2 **9** | 对 streak 透明是有意的；9 已在 A-17 修掉 |
| `pause` / `resume` | `goal.status` | 1 | 有意 |
| `compact` | **删 todo + 删 tick** | 4 5 **7** 8 | → A-18；7（花费）已修，且现在也被 `ensure_idle` 挡住 |
| `goal new/switch/rm` | 换掉整个 `state.json` | 全部 | 已被 `goals::ensure_idle` 挡住（runner 在跑就拒绝，除非 `--force`） |
| `reflect --apply` / `remember` | 只写 `NOTES.md` | — | 不碰账本 |
| `status` `stats` `context` `log` `doc` `doctor` `sessions` `next --peek` `replan`（不带 `--apply`）`hook-stop` | 只读 | — | 不碰账本 |

三条新的，全部复现成立。

### A-17（高）人插一句 `zloop feedback`，一轮**失败**的宿主就被记成「写回了」——连续失败停机整个失效 — 已修

结算那一步（`runner.rs:1069-1091`）的注释写的是 "did the host write back?"，
代码问的却是「这段时间里账本长了没长」：

```rust
for i in ticks_before..st.ticks.len() {
    if t.outcome != "noop" { wrote = true; }        // ← 有人加了条 tick
}
if !wrote && !rate_limited && !result.interrupted {
    tick::record(st, "fail", Some(&todo.id), &note, &who)?;   // ← 只有这里才记 fail
}
```

`noop` 被排除掉了（A-16 顺手补的），但同一个窗口里还能落进 `feedback` / `edit` /
`replan` 三种 tick，**每一种都是交互式命令记的、每一种都不是宿主的写回**。
而 `zloop feedback` 恰恰是文档教人「跟正在跑的循环说话」的那条路——
交接包里那句「用户对上一轮的反馈」就是这么来的，撞上的概率不低。

一串下去：不记 `fail` → `fail_streak` 恒为 0 → 第 4 项那道停机闸永远不触发。

A/B 实测（`scripts/repro-a17-interactive-write-masks-a-failed-round.sh`），
同一个每轮必败的假宿主、同一份 `max_fail_streak=2`，唯一差别是有没有人插话：

```
=== 场景 A：宿主每轮都失败，没人插话 ===
  起了 2 轮 ｜ 账本里 fail：2 条 ｜ journal 每轮的 wrote_back：false false
  runner: runner: stop (fail_streak)

=== 场景 B：一模一样，只是人每隔 1 秒敲一句 zloop feedback ===
  起了 7 轮 ｜ 账本里 fail：0 条 ｜ journal 每轮的 wrote_back：true true true true true true true
  runner: 还在跑（20 秒都没停）
  runB.log: runner: round 1 written back        ← 宿主 stderr 明明是 "host blew up"
```

**还带两个后果**，都实测到了：

1. `--git-commit` 会跟着动。`wrote_back` 为真就走 checkpoint，于是失败那一轮留在树里的
   半成品被提交，commit message 取的是「最后一条 tick 的 note」= **人写的那句反馈**：

   ```
   $ git log --oneline -1
   924e226 zloop t1: 先别动 work.txt
   $ git show --stat HEAD
    half-written.rs | 1 +      ← 宿主没写完就 exit 1 的那个文件
   ```

2. 这一轮的花费/轮数/日志被挂到人那条 `feedback` tick 上（`runner.rs:1093-1113`
   取的是 `ticks.last_mut()`），账本从此对不上号。

修的方向（下一轮做，不在本轮）：`wrote_back` 不能靠「账本长没长」推，得认人——
只把**宿主这一轮的会话**记的 tick 算数，或者反过来只认 `done/progress/fail/block`
这四种真写回的 outcome。后者更稳：`zloop done` 从人的会话里敲同样算写回，本来就该算。

**修法**（t24）——按上面第二条走，一共三处，缺一条闸就还是关不上：

1. **`wrote_back` 只认写回的 outcome**（`runner.rs:1073` + `tick::is_writeback`）。
   新增 `tick::WRITEBACK = ["done","progress","fail","block"]`，就是 `zloop done`
   的四个出口。判据从「这段时间里账本长了没长」变成「这段时间里有没有人把这一轮结掉」。
2. **这一轮的花费挂在结掉它的那条 tick 上**（`runner.rs:1095`）。`ticks.last_mut()` 换成
   「轮内最后一条写回 tick」；一条都没有就什么都不挂——上面第 2 个后果。
3. **`feedback` 不再无条件清 `fail_streak`**（`tick::fail_streak`）。只修 1 是不够的：
   fail 确实开始记了，但人每插一句就把两次 fail 隔开，尾部连续数永远到不了上限，
   实测仍是「起了 7 轮、账本里 6 条 fail、一直不停」。现在的规则是**只有循环已经停在
   `fail_streak` 上**（前面攒的 fail ≥ `max_fail_streak`）那句反馈才清零——README
   里那段实测（3 次 fail → `WAIT (fail_streak)` → `feedback` → `RUN`）一字不差照旧成立，
   变的只是「还在跑的时候补一句话」不再算"失败被解决了"。
   为此 `fail_streak(ticks)` 改成 `fail_streak(state)`（要读 `policy.max_fail_streak`），
   实现也从"从尾往前扫"改成"从头往后走"——要判一条反馈写下时循环停没停，
   得知道它**前面**攒了几条 fail。

修完同一个脚本：

```
=== 场景 A：宿主每轮都失败，没人插话 ===
  起了 2 轮 ｜ 账本里 fail：2 条 ｜ journal 每轮的 wrote_back：false false
  runner: runner: stop (fail_streak)

=== 场景 B：一模一样，只是人每隔 1 秒敲一句 zloop feedback ===
  起了 2 轮 ｜ 账本里 fail：2 条 ｜ journal 每轮的 wrote_back：false false
  runner: runner: stop (fail_streak)

[OK] 两边都停在 fail_streak：交互式命令写的 tick 不再被当成宿主的写回
```

回归测试三条，撤掉对应那处修复就变红：

| 测试 | 盯住的是 | 撤掉之后 |
|---|---|---|
| `runner_test::a_humans_feedback_cannot_mask_a_failed_round` | 1 + 3：宿主每轮失败、每轮自己敲一句 feedback → 仍记 2 条 fail、`wrote_back=false`、停在 `fail_streak`；`--git-commit` 一个 checkpoint 都没有，半成品留在树里 | 60 秒跑不完（`round 1 written back · host blew up` 无限循环） |
| `runner_test::the_rounds_cost_lands_on_the_write_back_not_on_a_humans_note` | 2：写回之后人再开口，花费/轮数/时长记在 `done` 那条上 | `left: (None, None, None)` |
| `tick_test::feedback_mid_run_does_not_disarm_the_fail_brake` | 3：反馈插在 fail 之间不清零；停下之后的反馈照旧清零 | `第 1 句反馈把失败计数清零了 left: 0 right: 1` |

**没顺手改的**（同一张表上、同型、留给后面）：`edit` tick 也会打断 `fail_streak`
（README 明说「任意 `edit` 都会重置计数」，而且 `edit` 至少意味着计划真的动了，先留着）；
`feedback` 对 `progress_streak` 的无条件清零是第 5 项上一模一样的形状，没在本轮动。
→ 这两条就是第 11 节的 **A-20 / A-21**，都已复现并修掉（「`edit` 意味着计划真的动了」
只在改的**正是当事那条 todo** 时成立——改别的 todo 时计划动的不是这条活）。

### A-18（中）`zloop compact` 把花费一起归档走，`max_total_usd` 静默复位 — 已修

A-16 / A-17 是交互式命令**写进**账本串了调度，这条是**从账本里删掉**东西串了调度。
runner 读的所有累计量（第 4 / 5 / 7 / 8 项）都是从 `ticks` 现算的，
谁动了 `ticks` 谁就动了这些闸。

`cmd_compact`（默认 `--keep-days 7`）把「终态且够老」的 todo 连同**它名下的所有 tick**
搬进 `archive/`。花费就记在 tick 的 `cost_usd` 上，于是：

```
=== 整理之前 ===
  should_run=False reason=budget
   ⛔ 已停   跑了 1 轮 · 花了 $9.50（上限 $5.00）
  $ zloop start → start: 没启动——runner 起来第一轮就会退出（budget）。

=== 人做了一次例行整理：zloop compact ===
  compacted 1 todos and 1 ticks → .zloop/archive/compact-….json

=== 整理之后 ===
  should_run=True reason=ready        ← 预算闸没了
```

（`scripts/repro-a18-compact-resets-budget-cap.sh`）

`max_total_usd` 的语义是「这个目标一共只准花这么多」，compact 把它悄悄改成了
「最近 7 天只准花这么多」。而且**没有任何痕迹**：整理之后 `zloop status` 连花过钱
这件事都不再显示，人不会知道自己刚刚给循环提了额。

而且它**不受 `ensure_idle` 保护**：runner 正在跑的时候敲 `zloop compact` 照跑不误
（同一时刻 `zloop goal new` 会被拒——`zloop: runner 正在跑（pid …）：换目标会让它中途换活`）。
两条命令动的都是 runner 下一轮要读的账，闸只装了一条。

**修法**（两半，缺一半都不算修完）：

1. **账不跟着 tick 走**。`state.json` 多一个 `archived`（`state.rs`，
   `{ticks, cost_usd, at}`，空的时候不落盘），`compact` 搬走多少花费就往里记多少；
   新的 `tick::spent_total(state)` = 现有 tick 之和 + `archived.cost_usd`，
   预算闸（`tick.rs:decide`）和四个显示花费的地方（`status` / `context` / `stats` /
   `start` 的预检说明）全部改走它。`spent_usd(ticks)` 原样留着——它答的是
   「**这些 tick** 花了多少」，是另一个问题。
2. **整理这件事本身要被挡一下**。`compact` 删 tick = 改 `fail_streak` /
   `progress_streak` / 花费 / 配额窗口这四道闸的输入，和 `goal switch` 同一类，
   所以走同一个 `goals::ensure_idle`（runner 在跑、或有轮次没写回就拒绝，`--force` 放行）。
   `ensure_idle` 顺带加了个 `why` 参数：拒绝的时候得说清楚是"换目标会让它中途换活"
   还是"整理账本会动它正在读的轮次记录"。

外加一行痕迹：带走了钱就在输出里说出来
（`归档里带走 $9.50 花费，已记进累计账（累计 $9.50）：policy.max_total_usd 不受整理影响`）。

```
$ sh scripts/repro-a18-compact-resets-budget-cap.sh
  === 整理之后 ===
    should_run=False reason=budget
     ⛔ 已停   跑了 0 轮 · 花了 $9.50（上限 $5.00）      ← 轮次归档走了，钱没有
  [OK] 整理不再抹掉花费：预算闸还在
```

| 测试 | 盯住的是 | 撤掉修复之后 |
|---|---|---|
| `cli_test::compacting_the_ledger_does_not_disarm_the_budget_cap` | 撞到上限的目标整理一次之后：`next --peek` 仍是 `budget`、`start` 仍拒绝、`status` 仍显示 $9.50；整理两次不会把同一笔钱记两遍 | `整理之后预算闸没了：… "reason": "ready"` |
| `cli_test::compacting_is_refused_while_the_runner_is_running` | runner 在跑 / 有轮次没写回时 `compact` 被拒且不动账本，`--force` 才放行 | `runner 跑着还让整理：compacted 1 todos and 1 ticks → …` |

**没顺手改的**：`rounds`（「跑了几轮」）照旧只数现有 tick，整理之后会掉下来——
`status` 和 `stats` 共用同一个 `tick::rounds`，两处一起改才不会自相矛盾，而
`stats` 的返工率是 `rework/rounds`，分母加了归档、分子没加就成了另一个数。
它不是停机闸（没有哪道闸读 `rounds`），留给一条单独的活。

### A-19（中高）人留一句反馈，下一轮无头 runner 就 `--resume` 进人的对话里 — 已修

`pick_session`（`runner.rs:440`）挑「上一轮的会话」时只看两件事：host 对不对、
（`--resume todo` 模式下）todo 对不对。**它不看这条 tick 是谁记的。**

而 `zloop feedback` / `zloop edit` 会把调用者的 `CLAUDE_CODE_SESSION_ID` 原样记进 tick
（`session::detect()`）。于是「人在自己的 Claude Code 会话里给这条 todo 留句话」
= 「把自己的会话 id 挂到这条 todo 名下」，而 runner 的默认 `--resume todo` 正好去捡它：

```
=== 人在自己的 Claude Code 会话（HUMAN-SESSION-9999）里给 t1 留了一句反馈 ===
  $ zloop feedback → feedback → t1：顺便说一句：先别动 x.rs
=== runner 无头跑两轮 ===
  runner: round 1 → t1 [claude] resume HUMAN-SESSION-9999      ← 人的会话
  runner: round 2 → t2 [claude]                                 ← t2 没人留过话，干净
=== 假宿主实际收到的 --resume ===
  第 1 轮：--resume [HUMAN-SESSION-9999]
  第 2 轮：--resume []
```

（`scripts/repro-a19-runner-resumes-a-humans-session.sh`）

后果不是「记错一个 id」这么轻：`claude -p --resume` 上去之后，这一轮的提示词接在
人那段对话**后面**跑——上下文全是不相干的、token 按整段转录计费、产出还写进人的转录里。
人正开着那个会话的话，两边同时往一条对话里写。

顺带一提，这一条在写 A-17 的复现脚本时是**自己跳出来的**：脚本里的 `zloop feedback`
继承了当时那个 Claude Code 会话的 id，runner 的日志直接打出
`resume 870f6118-…`——那正是当时跑审查的那个会话。

**修法**：`pick_session` 的判据从「host 对不对」改成「这条 tick 是不是宿主结掉一轮留下的」——
复用 A-17 装的那个判据 `tick::is_writeback`（`done` / `progress` / `fail` / `block`）。
别让「人说过话」等于「人跑过这一轮」：

```rust
let host_round = |t: &&state::Tick| {
    t.host.as_deref() == Some(host.as_str()) && t.session.is_some() && tick::is_writeback(&t.outcome)
};
```

谱系没被这道过滤削掉：宿主超时 / 崩掉时 runner 自己补的那条 `fail`
（`runner.rs:1093`，`who` 用的是本轮宿主报回来的 session）也是 `WRITEBACK` 成员，
所以「上一轮没写回，下一轮接着同一个会话再试」照旧成立。

同一道过滤顺带挡掉另外两种「有 session id、但不是上一轮干活的那个」——两条都只在
`--resume all` 下够得着（`--resume todo` 那支被 `t.todo` 先滤掉了）：

| tick | 谁记的 | 修之前 `--resume all` 会怎样 |
|---|---|---|
| `noop` | `zloop next`（cli.rs:761），人在自己会话里敲的 | 接进人的会话 |
| `reflect` / `replan` | runner 自己插的那轮，`--resume None` 起的一次性会话 | 下一轮工作接到回看的上文里 |

复现脚本和回归测试：

```
$ sh scripts/repro-a19-runner-resumes-a-humans-session.sh
  第 1 轮：--resume []
  第 2 轮：--resume []
[OK] runner 没有 resume 人的会话：交互式命令留下的 session id 不再被当成上一轮的宿主会话
```

| 测试 | 盯住的是 | 撤掉修复之后 |
|---|---|---|
| `runner_test::a_humans_feedback_is_not_a_session_to_resume` | 人给 t1 留一句 `feedback` 后跑两轮（默认 `todo` 模式 + `--resume all` 各一遍）：argv 里不许出现人的 session id；同时 `--resume all` 下第 2 轮仍要接上 `sess-t1` | `runner 跑进了人的对话里（[]）：resume=HUMAN-9999` |

### 11.3 检查过、确认不是问题的

- ~~**`edit` / `feedback` 打断 fail / noop / progress 三条 streak**：有意设计，`tick.rs:14`
  写明了理由——「人开口说话正是『停下来等人』该等到的东西」。第 4 / 5 项就该被它打断。~~
  **这条判断是错的**，见第 11 节（A-20 / A-21）：代码里的注释说的是「停下来等人之后人开口」，
  代码做的是「人任何时候开口」，两者只在循环**已经停了**的时候等价。本轮把这条从
  「不是问题」挪回发现清单——`noop` 那条仍然成立（它不是停机闸，且 A-16 之后 runner 不读它）。
- **`reflect` / `replan` tick 对 streak 透明**：有意，`tick.rs:22` 写明「插一轮反思不代表
  失败被解决了」。
- **`goal new` / `switch` / `rm` 换掉整个 `state.json`**：`goals::ensure_idle` 已经挡住了
  （runner 在跑、或有轮次没写回，都拒绝，除非 `--force`）。试着在 runner 跑的时候切目标，
  被拒得干干净净。
- **`next` 记的 `noop` 混进 `wrote_back`**：`outcome != "noop"` 那一行已经排除。
  它是对的，但**只对了 noop 一种**——A-17 说的就是剩下三种。
- **`held_by_other` 挡不住 runner**：有意且必须（`tick.rs:108` 有整段说明和四种在场组合的表），
  runner 自家的 `claude -p` 子进程要靠这条放行才进得来。

---

## 12. 第九轮：A-17 那张表上同型的最后两条

A-17 修完时留了一句「没顺手改的」：`edit` 也会打断 `fail_streak`、`feedback` 对
`progress_streak` 的无条件清零是一模一样的形状。这一轮把这两条一起复现、一起修掉。
两条都是同一句话的两个变体——**「人在另一个终端敲一条交互式命令，就把无头 runner 的停机闸拆了」**。

复现脚本：`scripts/repro-a20-a21-another-terminal-disarms-the-brakes.sh`（四个场景，
两对 A/B 对照；退出码 1 = 至少一条复现）。

### A-20（高）人顺手整理 backlog（`zloop edit` 改**别的** todo），连续失败停机这道闸就被拆了 — 已修

`tick::fails_in_a_row` 的最后一个分支是 `_ => n = 0`，`edit` 落在里面：

```rust
match t.outcome.as_str() {
    "fail" => n += 1,
    o if transparent(o) => {}
    FEEDBACK => { if forgive_at > 0 && n >= forgive_at { n = 0; } }   // A-17 收窄过的
    _ => n = 0,   // ← done / progress / block / edit 一视同仁
}
```

`edit` tick 全仓只有一个写入点：`cli.rs:1033`，也就是**人敲的 `zloop edit`**
（`replan --apply` 改计划记的是 `replan`，对 streak 透明，不走这条）。而 README 明说
「任意 `edit` 都会重置计数」——这句在**循环已经停下**的场景里是对的，问题是代码没区分
停没停，也没区分改的是**哪一条** todo：无头 runner 正在 t1 上一轮一轮失败，人在另一个
终端把 t7 的文字改一改、把 t9 推后，t1 的失败计数就归零了。

A/B 实测（同一个每轮必败的假宿主、同一份 `max_fail_streak=2`，唯一差别是有没有人改 **t2**）：

```
=== A-20 场景 A：宿主每轮都失败，没人插话 ===
  起了 2 轮 ｜ 账本里 fail：2 条
  runner: runner: stop (fail_streak)

=== A-20 场景 B：一模一样，只是人每隔 1 秒 zloop edit t2（另一条 todo！）===
  起了 7 轮 ｜ 账本里 fail：6 条
  runner: 还在跑（20 秒都没停）
```

**修法**：`fails_in_a_row` 顺带记住这串连续失败落在哪几条 todo 上（`failing`），
`edit` 分支单独拿出来，两种情况才清零：

1. `edit` 改的**就是**正在失败的那条 todo —— 活真的换了，之前的失败不再算数
   （README 教的出口 `zloop edit t3 --text …` 一字不差照旧成立）；
2. 循环**已经停在 `fail_streak` 上** —— 人是在回应一个停着的循环，和 `feedback` 同一条规矩。

没记 todo id 的 `edit`（手改过的账本）按「改的是别的活」处理：宁可多认不可漏认。

### A-21（高）人插一句 `zloop feedback`，同一条 todo 原地踏步那道闸就永远数不到上限 — 已修

`tick::progress_streak` 从尾往前扫，`_ => break`，于是任何一条 `feedback` 都把它断掉。
这就是 A-17 后半截在 `fail_streak` 上修掉的形状，换了一条 streak 而已，
而 `zloop feedback` 正是文档教人「跟正在跑的循环说话」的那条路。

A/B 实测（假宿主每轮都 `zloop done t1 --outcome progress`，`max_progress_streak=2`）：

```
=== A-21 场景 C：宿主每轮都 progress 原地踏步，没人插话 ===
  起了 2 轮 ｜ 账本里 progress：2 条
  runner: runner: stop (progress_streak)

=== A-21 场景 D：一模一样，只是人每隔 1 秒 zloop feedback t1 ===
  起了 9 轮 ｜ 账本里 progress：8 条
  runner: 还在跑（20 秒都没停）
```

后果比 A-17 更"安静"：宿主每轮都在**正常写回**，日志、花费、轮次全都对，
只是同一条 todo 永远完不了——`max_progress_streak` 这道「这条活太大了，停下来让人拆」的闸
从此不响，长跑会一直在一条 todo 上烧到撞 `max_runs` 或 `max_total_usd`。

**修法**：`progress_streak(ticks, todo_id)` 加一个 `forgive_at` 参数
（调用处传 `policy.max_progress_streak`），实现也从"从尾往前扫"改成"从头往后走"——
和 `fails_in_a_row` 一样，要判一条 `feedback` 写下时循环停没停，得知道它**前面**攒了几轮。
规矩和 A-20 完全一致：

| 这一轮的 tick | 还在跑 | 已经停在 `progress_streak` 上 |
|---|---|---|
| `feedback`（任意 todo） | 不清零 | 清零 |
| `edit` 改的**就是**这条 todo | 清零（README 的出口「拆小它」） | 清零 |
| `edit` 改的是别的 todo | 不清零 | 清零 |
| `noop` / `reflect` / `replan` | 透明 | 透明 |
| 其它（`done` / `fail` / `block` / 别的 todo 的 `progress`） | 断掉 | 断掉 |

修完同一个脚本四个场景全停：

```
[OK]   A-20：改别人的 todo 不再拆掉连续失败这道闸（A=stop (fail_streak) / B=stop (fail_streak)）
[OK]   A-21：反馈不再拆掉原地踏步这道闸（C=stop (progress_streak) / D=stop (progress_streak)）
```

场景 D 修完是 4 轮才停（A/C 是 2 轮）：人每秒插一句，总有几句正好落在"计数刚够到上限、
`decide` 还没来得及看"的缝里，按第 1 条规矩那是合法的清零。**闸关得上就行**——
它本来的语义就是「停下来等人，人回应了就再试一次」。

回归测试两条，撤掉对应那处修复就变红：

| 测试 | 盯住的是 | 撤掉之后 |
|---|---|---|
| `tick_test::an_edit_on_another_todo_does_not_disarm_the_fail_brake` | A-20：改 t2 不清 t1 的失败计数；停下之后改 t2 才清；改 t1 照旧随时清 | `第 1 次改别的 todo 把失败计数清零了 left: 0 right: 1` |
| `tick_test::feedback_mid_run_does_not_disarm_the_progress_brake` | A-21：反馈插在两轮 progress 中间不清零；停下之后清；`edit` 改这条 todo 随时清、改别的不清 | `第 1 句反馈把原地踏步计数清零了 left: 0 right: 1` |

### 12.1 这一类到此为止

「交互式命令写进账本的东西串进 runner 的判断」这一类（A-16 → A-17 → A-18 / A-19 → A-20 / A-21）
到这一轮为止，`tick.rs` 里三条 streak 的规矩统一成了一句话：

> **人写的 tick（`feedback` / `edit`）只有在循环已经停在这条 streak 上时才清零；
> 还在跑的时候，只有「`edit` 改的正是当事的那条 todo」算数。**

`noop_streak` 不在此列：它不是停机闸，A-16 之后 runner 也不读它。

A-19（`pick_session` 认错会话）跟 streak 无关，但归到同一句话下面：它串的不是停机判断，
是「这一轮 `--resume` 谁」，判据换成同一个 `tick::is_writeback` 就修好了——
**人写的 tick 不是宿主跑过的一轮**，三条 streak 和会话谱系都按这一句办。
A-18（`compact` 抹掉花费）是这一类的另一头——**从账本里删东西**同样串了调度，
它的修法也就成了另一句话：**归档只该让账本变小，不该让账变少**（累计花费单独存一份），
而且**动账本的命令要和 `goal switch` 走同一道闸**（`ensure_idle`）。这一类到此全部修完。

## 13. 第十轮：「永远等不到」的第三种形状

A-9 修完时留了一句话没往下问：`dangling_blocked_by`（依赖不存在）和 `dep_cycle`（依赖成环）
都在报同一个后果——**这条 todo 永远轮不到**。那这个后果还有没有第三种走法？有，而且不用手改文件。

### A-22（中高）依赖一条已延后的 todo：卡死的形状一模一样，doctor 却退 0 — 已修

两条真命令就能走到：

```
zloop edit t2 --blocked-by t1
zloop edit t1 --status deferred
```

此后：

```
zloop next --peek --json → should_run=false  reason=blocked  interval_min=10
zloop status             → 清单 0/1 完成 · 1 条延后 ｜ t2 的进展写着「⏳ 等 t1」
zloop doctor             → 没发现问题（exit 0）
```

`is_terminal` 把 `deferred` 和 `done` 一视同仁，所以 t1 不再进 `open_ordered`，
**永远派不出去**；而 `is_executable` 要求依赖 `status == "done"`，t1 停在 `deferred` 上
就永远满足不了。两头一夹，t2 就是 A-9 那个「等到天荒地老」的状态——只是这一次
依赖那条还好端端地躺在清单里，`dangling_blocked_by`（判的是「id 找不到」）够不着它，
`check_dep_cycles`（判的是「回边」）也够不着。面板上更误导：`status` 写的是「等 t1」，
像在正常排队，其实前面那位已经走了。

同一条检查的另一半是**手改进来的野状态**：`STATUSES` 只有四个词，
把 `state.json` 里某条 todo 改成 `"cancelled"`（loopx 有这个状态，从别处抄配置很容易带进来）
之后它既**不是** terminal（还占着 `remaining`，`status` 面板照常列它）、
又过不了 `is_executable` 的 `status == "open"`——它自己跑不了，还把依赖它的那条一起钉死。
实测 `remaining: 2`、`reason: blocked`、doctor 沉默退 0。

**要不要并进 `dangling_blocked_by`？不并**，理由是出口动作不一样：
`dangling` 的依赖已经没了，只能改成别的 id 或断开；这里依赖还在，
最常见的正确出口是**把它捡回来**（`zloop edit t1 --status open`）。一句 fix 说不清两件事，
`kind` 也是脚本要拿来分流的稳定标识，混在一起等于把两种处置合成一个词。
于是新增 `dead_blocked_by`（`doctor.rs::check_ledger` 第 3b 块），
和 `dangling_blocked_by` 并列、判据是同一个问题的另一半：
**依赖在，但它还有没有机会走到 `done`**——`can_still_finish` 只放行 `open`（会被派出去）、
`blocked`（等的是人，人一答就回队列）、`done`（本来就满足）。

严重度取 **Error**，和两个同型的邻居一致：`dangling_blocked_by` 不分情况都是 Error，
`dep_cycle` 在「环上还有活着的 todo」时是 Error。这里等着的那条按定义就是活的
（terminal 的 todo 一开始就跳过了），所以不再分档。
`dep_cycle` 那条 warn（全 deferred 的环）也不会被这条抢走：环上每个点都是 terminal，
第一步就被跳过，两条检查不重叠。

回归测试三条，撤掉 `can_still_finish` 的判据（改成恒 `true`）前两条立刻变红：

| 测试 | 盯住的是 | 撤掉之后 |
|---|---|---|
| `doctor_test::depending_on_a_deferred_todo_is_reported_like_a_dangling_one` | 先钉住 `reason=blocked` 这个前提，再要求 doctor 报 `dead_blocked_by` + exit 1；`edit t1 --status open` 之后立刻闭嘴、`next` 重新有活干 | `[]`（findings 空） |
| `doctor_test::depending_on_a_todo_with_an_unknown_status_is_reported_too` | 手改成 `"cancelled"` 的那一半，且要把野状态原样印进 `what` | `[]` |
| `doctor_test::a_live_dependency_is_not_a_dead_end` | 反向：依赖还开着 / 依赖在等人（`done --block`）/ 两条都 deferred，一个字都不该报，doctor 退 0 | 照常绿（它防的是误报） |

## 14. 第十一轮：两条小尾巴

### T36-①（低）`tests/scratch_t33.rs` 被误提交进仓库 — 已清

t33 那一轮留下的观察用脚手架，文件里两行注释自己写着「未被 git 跟踪、该 `rm` 掉」，
结果 `git add -A` 连它一起带进了 71a74d6。内容只有那两行注释，编译产物是一个空的
测试二进制——不影响任何结果，但它是一句**写在仓库里的假话**（"未被 git 跟踪"），
下一个人照着读会以为工作区脏了。`git rm tests/scratch_t33.rs`，没有别的动作。

### T36-②（中）`status` 对「永远等不到」和「正常排队」用同一个词 — 已修

A-22 的收尾里写了一句「面板上更误导」，当时只修了 `doctor`。这一轮把那句话补完。

`status` 的进展列对**三种命完全不同**的等待印同一个东西——一行灰的 `⏳ 等 tN`：

| 依赖那条的状态 | 会不会有 done 的一天 | 修复前的进展列 | doctor |
|---|---|---|---|
| `open` / `blocked` | 会，迟早轮得到 | `⏳ 等 t1` | 不报（正常形状） |
| `deferred` | 不会（不进 `open_ordered`） | `⏳ 等 t4` | `dead_blocked_by`，exit 1 |
| 手改的野状态（`cancelled`） | 不会（过不了 `is_executable`） | `⏳ 等 t4` | `dead_blocked_by`，exit 1 |
| 不在清单里（`compact` 搬走了） | 不会 | `⏳ 等 t1` | `dangling_blocked_by`，exit 1 |

复现（四条真命令，不手搓状态）：

```
zloop plan --add "[P0] 做基础" --add "[P1] 等基础" --add "[P2] 等一条延后的" --add "[P2] 会被延后"
zloop edit t2 --blocked-by t1 ; zloop edit t3 --blocked-by t4 ; zloop edit t4 --status deferred
zloop status   → │ 2 │ t2 │ 等基础       │ ⏳ 等 t1 │      ← 正常排队
                 │ 3 │ t3 │ 等一条延后的 │ ⏳ 等 t4 │      ← 永远轮不到，长得一模一样
zloop doctor   → ✗ t3 依赖 t4（已延后）…（exit 1）
```

**评估结论：标出来。** 两块屏的读者不是同一批人——`doctor` 是**起了疑**才会去跑的，
而 `status` 是**每天都看**的那一块。三行长得一样，人根本没有起疑的由头，
也就永远走不到 `doctor` 那一步；`next` 那头只会说 `blocked` + "隔一阵重试"，
更像在正常等待。这不是审美问题：它决定的是**这个问题要多久才被发现**。

改法（`cli.rs::status`）：

- 判据**不重写一份**，`can_still_finish` 从 `doctor.rs` 提到 `todo.rs` 变公开，
  两处共用一个定义。分两份写过一次就会走散：一块屏幕报警、另一块说一切正常。
- 死等的那行印 `⛔ 等不到 t4`，paint 用 3（黄）。**换词不换色不行**——管道 / `NO_COLOR`
  下颜色一个字都不剩，得让文本自己把话说完。
- 底下挂一条出口命令（`↳ 解开敲 …`），和「等你回话」的 `↳ 答完敲 …` 同一套。
  给哪条命令跟着 doctor 走：依赖还在就 `edit <dep> --status open`（捡回来），
  依赖已经不在了只能 `edit <t> --blocked-by ''`（断开）——**不能反过来**，
  让人去改一条已经不存在的 todo 是死路。
- 顺带修了溢出行：窄窗口下装不下的命令会被攒到表外印，而那段代码把引导词写死成
  「答完敲」。多了第二种命令之后，`解开敲 zloop edit t4 --status open` 会被印成
  「t3 答完敲 …」，指着人去回一个没人问过的问题。改成 `spill` 自带引导词。

回归测试 `cli_test::status_tells_a_dead_wait_apart_from_a_normal_queue`：
先用 `doctor` exit 1 钉住「这确实是死等」的前提，再验四件事——
活依赖仍是 `⏳ 等 t1`、死依赖变 `⛔ 等不到 t4` 且不再出现 `⏳ 等 t4`、
出口命令在、捡回来之后 `⛔` 立刻消失；野状态和 `compact` 搬走两种走法各验一遍
（后者要求给的是「断开」而不是「捡回来」）。外加表格宽度一致 + 46/60/80/100 列不折行。
撤掉这个分支立刻变红：

```
panicked at tests/cli_test.rs:1248: 等一条已延后的依赖要说出来:
  │    3 │ t3 │ 等一条延后的 │ ⏳ 等 t4 │
```

### T37（中）「永远等不到」只在 `status` 一块屏上说了 — 已修（并补上 t36 漏判的那一半）

t36 只收了 `status` 一处。评估另外三处紧凑清单（`context.rs` / `prompt.rs::render_md` /
`cli.rs::cmd_edit` 的回显）时发现，**t36 自己的判据也漏了一种**，于是一起收口。

#### ① `status` 拿「第一条没 done 的依赖」去判死活

t36 的写法是先取 `pending_dep`（第一条没 done 的依赖），再问这一条死没死。
死依赖排在活依赖**后面**就整条漏掉——`doctor` 那边是把 `blocked_by` 整条扫完的。

```
zloop plan --add "[P1] a" --add "[P1] b" --add "[P1] c" --add "[P1] d"
zloop edit t4 --status deferred
zloop edit t2 --blocked-by t1,t4        # t1 还开着，t4 已延后
zloop doctor  → ✗ t2 依赖 t4（已延后）…（exit 1）
zloop status  → │ 2 │ t2 │ b │ ⏳ 等 t1 │   ← 照旧是「正常排队」
```

同一份 state，一块屏退 1 大喊永远轮不到，另一块说在排队——正是 t36 要修的那个病，
只是换了个依赖顺序就复发。**这类判据要问「整条 `blocked_by` 里有没有」，
别问「第一条是不是」**：doctor 一开始就是这么写的，status 抄的时候抄窄了。

#### ② 另外三处紧凑清单还在印 `⏳t4`

| 读者 | 修复前 | 危害 |
|---|---|---|
| `zloop context` 的「待办」段 | `- [ ] t3 [P1] c ⏳t4` | **模型每轮读的就是这一段**：一条永远轮不到的 todo 看着像在排队，模型接着做别的、谁也不去解 |
| `zloop status --md` | `- [ ] \`t3\` [P1] c ⏳t4` | state 的镜像，三份清单对同一条 todo 说不一样的话 |
| `zloop edit` 的回显 | `t3 [P1] open c ⏳t4` | **造出这条死依赖的那一刻**唯一会被读到的一行，却说"排上了" |

`edit` 那一处最要命：`--blocked-by` 只挡自依赖和不存在的 id（A-9），
依赖一条**已延后**的 todo 一路放行、回显还给个 `⏳`，人就走了。

**评估结论：三处一起收，判据只留一份。** 抽 `todo::dead_deps(state, todo) -> Vec<&str>`
（整条 `blocked_by` 扫完、去重、`user` 不算、终态的 todo 返回空）和
`todo::dead_dep_fix`（出口命令，方向不能反），四个读者共用：

- `status`：`⛔ 等不到 t4` + `↳ 解开敲 …`（原样，只是判据换成扫全部依赖）
- `context`：`⛔等不到 t4（zloop edit t4 --status open）`——**出口命令直接带在行里**。
  读它的是模型，只说"坏了"它还得再查一轮状态才知道敲什么，多烧一轮。
- `status --md`：`⛔等不到 t4`（镜像文档，不带命令）
- `edit`：`⛔等不到 t4` + 次行 `↳ 解开敲 …`；**退出码仍是 0**——这条 `edit` 本身是
  成功的，改成非 0 会把脚本里的 `edit && …` 打断。

终态的 todo 返回空是特意的：`done` / `deferred` 的那条不在等谁，给它印「等不到」是噪音
（`render_md` 会把做完的也列出来，不加这一条就会出现「已完成但等不到 t3」这种话）。

回归测试两条：

| 测试 | 钉住什么 | 撤掉后 |
|---|---|---|
| `cli_test::a_dead_wait_reads_the_same_on_every_list` | 先用 `doctor` exit 1 钉前提；再验死依赖排在活依赖后面时 `status` 不再印 `⏳ 等 t1`、四处一起说 `⛔`、`context` 带出口命令、`compact` 走法给的是「断开」、捡回来后四处一起回到 `⏳` | `panicked … 造出死依赖的那一刻就该说: t2 [P1] open 等两条 ⏳t1,t4` |
| `cli_test::a_finished_todo_is_not_waiting_on_anyone` | 反向：自己已了结的那条不许印 `⛔` | 去掉 `dead_deps` 的 `is_terminal` 早退 → `panicked … 自己已经了结了，不在等谁: t2 [P1] deferred 二 ⛔等不到 t3` |

第二条测试在修复前也是绿的（旧代码从不印 `⛔`）——它防的不是老 bug，是这次改动
自己的过度报警，所以验红要靠**撤掉那句 `is_terminal` 早退**，不是撤掉整个修复。

**留下的一条**（记进 `--next`）：`zloop edit t4 --status deferred` 会把「所有依赖 t4 的
todo」一起判死刑，而回显只讲 t4 自己，一个字都不说被它连累的那几条。反向扫一遍
`blocked_by` 就能说，但那是另一处改动，这一轮不顺手做。

---

## 15. 第十二轮：`compact` —— 同一个形状，但撤不回来

### T39（中高）`compact` 把还有人依赖的那条搬进归档，等它的那几条就此永远等不到 — 已修

`edit <dep> --status deferred`（T38）之后，「一条命令判死一片」的第二处是 `zloop compact`。

```
zloop plan --add "[P0] 做基础" --add "[P1] 等基础" --add "[P1] 也等基础"
zloop edit t2 --blocked-by t1 ; zloop edit t3 --blocked-by t1
zloop next ; zloop done t1 --note ok --approach 做了a
zloop doctor                     → exit 0（一切正常）
zloop compact --keep-days 0      → compacted 1 todos and 1 ticks → …/compact-….json
zloop doctor                     → ✗ t2 依赖 t1，但没有这条 todo——它永远轮不到
                                   ✗ t3 依赖 t1，但没有这条 todo——它永远轮不到   （exit 1）
```

一次例行整理（`compact` 的定位就是"目标跑长了敲一下"，甚至能进 cron），把两条还开着的
todo 判成死等，而回显只讲被搬走的那条自己。**判断力和回显之间隔了一次 `doctor`**：
被连累的是谁，在搬走的那一刻反向扫一遍 `blocked_by` 就知道。

#### 为什么这一处不能照抄 T38 的做法（补一行提醒）

| | `edit <dep> --status deferred`（T38） | `compact`（T39） |
|---|---|---|
| 撤回 | `zloop edit t4 --status open`，状态还在清单里 | **没有命令能捡回来**：todo 进了 `.zloop/archive/compact-*.json`，zloop 没有 restore |
| 出口 | 「把依赖捡回来」和「断开依赖」都还在 | 只剩「断开依赖」`zloop edit t2 --blocked-by ''`——连"它当初依赖谁"也一起丢 |
| 谁在敲 | 人临时改主意 | 例行维护，可能在脚本/cron 里，回显只进日志 |

所以这一处的结论是**留下那一条不搬**，而不是搬走之后说一声：

```
compacted 1 todos and 1 ticks → .zloop/archive/compact-….json
  ⏸ 留下 1 条没搬：还有没做完的 todo 在等它们
     t1 ← t2,t3
  ↳ 搬进归档就再也捡不回来；等它们做完，或 zloop edit t2 --blocked-by ''
```

- **不是"有依赖就整个不整理"**：其余到期的照常搬（同一次里 `t4` 归档了）。
- **不是永久钉住**：等的人一做完 / 一了结，下一次 `compact` 自然带上它。
- **一条都没搬时不许说谎**：到期的全被人等着，印的是 `nothing compacted：到期的 N 条都还有人在等`，
  不是原来那句 `nothing to compact (no done/deferred todos older than N days)`。
- **本来就死的也留下**：等的是一条已延后的 todo 时 `doctor` 整理前就在喊了，但那种状态的出口是
  `edit <dep> --status open`；搬进归档就只剩「断开」。**整理不该把人的退路整理掉。**

#### 判据仍然只留一份

`todo::dead_if_removed(state, victims)` 不新写规则，而是把「搬走之后」的清单真的跑一遍
`dead_deps`（被搬走的 id 从此走 `None => true` 那一支），只挑其中指向 `victims` 的。
于是终态 todo 不在等谁、`user` 不算依赖、重复 id 只报一次这三条，和四张清单、和 `doctor`
自动一致——一串 `done` 的依赖链（t2 done 且 `blocked_by t1`）一次就整理干净，
因为 doctor 的两处检查同样跳过终态。

#### 回归测试

| 测试 | 钉住什么 | 撤掉后 |
|---|---|---|
| `cli_test::compact_keeps_a_todo_that_others_still_wait_on` | 整理完 `doctor` 仍退 0；点名 `t1 ← t2,t3` + 出口命令；没人等的照常归档；全被等着时说 `nothing compacted`；等的人一了结就搬走；`done` 的依赖链不互相钉住；本来就死的那条也留下 | 删掉 `old_ids.remove(id)` 那三行 → `panicked … 整理不许留下 doctor 认定的坏状态: compacted 2 todos and 2 ticks …`（doctor 退 1） |

**连带改的两处测试**：`doctor_test` 和 `cli_test` 里原本拿 `compact --keep-days 0` **造**
「依赖指着不存在的 id」这个状态，现在造不出来了，改成直接从 `state.json` 里抹掉那条
（`drop_todo_from_state`）。检查本身一个字没动：**坏文件不会因为新版本不再生产就消失**，
手改过的 state 和老版本留下的 `state.json` 仍然要被 `doctor` 认出来。

## 16. 第十三轮：`compact` 的另外两个受害者（T39 只清了三分之一）

T39 修的是 `blocked_by`。但 `dangling_in_progress` 这条 doctor 检查本身就说明：
**指着 todo id 的字段不止 `blocked_by` 一处**。这一轮把 `state.json` 里
「存了一个 todo id」的字段列全，逐个问一句「compact 搬走它指的那条会怎样」。

### 16.1 指针全集（`src/state.rs` 的 struct 逐字段过）

| 存 todo id 的地方 | compact 怎么对它 | 结论 |
|---|---|---|
| `Todo.blocked_by: Vec<String>` | 反向扫一遍，还有人等就不搬（`dead_if_removed`） | T39 已修 |
| `Tick.todo: Option<String>` | 跟着 todo 一起搬进归档 | **T40-① 有洞**：判老的是 *todo* 的年龄，不是 tick 自己的 |
| `InProgress.todo: String` | **没人看** | **T40-② 有洞** |
| `Tick.log: Option<String>` | 路径进归档，`.zloop/log/*.md` 文件留在原地 | 孤儿文件，读得到、不报错，不算缺陷（见 15.4） |
| NOTES.md 的经验 / 约定 | — | **不是受害者**：`notes::Lesson = (时间戳, 正文)`，压根没有 id 字段 |

`Goal.id` 是目标 id、`Policy` 全是阈值，都不指 todo。所以指针一共三处，
T39 之后还剩两处没人管——正好对应下面两条。

### 16.2 T40-①（中高）例行 `compact` 吃掉人今天刚留下的、还没人读过的反馈 — 已修（见 §17）

`compact` 挑 tick 的判据是 `old_ids.contains(tick.todo)`（`cli.rs:2209`）——
**只看它挂在哪条 todo 上，不看这条 tick 自己多老**。于是一条五秒钟前写下的
`feedback`，只要挂在一条 40 天前完成的 todo 上，就跟着一起进归档。

```sh
zloop next ; zloop done t1 --note n --approach a       # t1 完成于 40 天前
zloop feedback t1 "方向错了，下一轮先停下来问我"        # 今天，人留的话
zloop context | grep 方向错了       → ## 用户对上一轮的反馈（先处理这些）… 1 条
zloop compact --keep-days 30        → compacted 1 todos and 2 ticks → …/compact-….json
zloop context | grep 方向错了       → 0 条
```

三点让它比 T39 更难发现：

- **不需要 `--force`**，`ensure_idle` 一道闸都不响：这是最普通的例行整理，甚至能进 cron。
- **静默**：回显只说「2 ticks」，不说其中一条是人写给下一轮的指令。
  `doctor` 整理前后都退 0——归档里的东西不算「坏状态」，没有任何检查会去数它。
- **丢的正是协议里排第一的那个输入**：`zloop context` 把反馈放在「下一条」之前，
  skill 的原话是「交接包里有『用户对上一轮的反馈』就先按它调整这一轮的做法」。
  人说完话就走开了，下一轮的 agent 一个字都看不到，而且没人会知道少了一条。

`--keep-days 30` 这个组合是**正常用法**：反馈本来就常常是隔了几天回看时才留的
（`cmd_feedback` 自己都印「（t1 已经是 done；要让它重做：`zloop edit t1 --status open`）」，
说明"对终态 todo 留反馈"是设计内的路径）。

方向：搬 tick 的判据要多一条——**这条 tick 自己也得够老**，或者至少
`pending_feedback` 里的那些不许搬；一条都不该在人还没读到之前消失。

### 16.3 T40-②（中）`compact --force` 把在飞的那条搬走，`ensure_idle` 给的两条出口从此都退 2 — 已修（见 §17）

`cmd_compact` 先过 `goals::ensure_idle`，有 `in_progress` 就拦下。**但 `--force` 直接
`return Ok(())`**（`goals.rs:217`），而 `old_ids` 的挑选完全不看 `st.in_progress`。

前提两步都在正常用法里：`zloop edit` 从头到尾不碰 `in_progress`（`cli.rs:1037-1111`），
所以把在飞的那条改成终态，`in_progress` 就留在一条 `done` / `deferred` 的 todo 上；
这时 `compact` 会被 `ensure_idle` 拦住，而它给的出口人不想走（`done t1` 会记一条假的
完成、`edit t1 --status open` 会把刚做的延后撤销），于是加 `--force`。

```sh
zloop next                          # 派出 t1，in_progress = t1
zloop edit t1 --status deferred     # 人判它不做了；edit 不动 in_progress
zloop compact --keep-days 0 --force → compacted 1 todos and 1 ticks

zloop compact --keep-days 0
  → zloop: 有会话正拿着 t1 第 1 轮还没写回：先 `zloop done t1` 收尾
     （或 `zloop edit t1 --status open` 放回去），或加 --force
zloop done t1 --note x --approach y     → exit 2  done: unknown todo id "t1"
zloop edit t1 --status open             → exit 2  edit: unknown todo id "t1"
zloop doctor → ✗ 第 1 轮派出去的 t1 已经不在待办里了            （exit 1）
```

**闸和出口一起失效**：`ensure_idle` 是 `compact` / `goal switch` / `goal new` 共用的那一道，
从此这三条命令全要 `--force` 才动得了，而它印的两条自救命令都是「unknown todo id」。
`zloop status` 还照旧印着 `阶段 claude 正在做 t1 · 第 1 轮` 和
`写回 zloop done t1 …`——一条保证失败的命令。

不判高的两个理由：要 `--force`（人明确越过了闸），而且 `zloop next` 再派一次活会顺手
把 `in_progress` 覆盖掉，算半条自愈路。但 `doctor` 报的是 err、给的修法是
「手工把 state.json 里的 in_progress 删掉」——**zloop 自己造出了一个只能手改文件才能
收拾的状态**，而 T39 刚刚为同一个理由（归档捡不回来）选择了「拦下来」。

方向：和 T39 同一个形状——`in_progress.todo` 不许进 `old_ids`，`--force` 也不许。
`--force` 的语义是「我知道有人在跑，账我认」，不是「把在飞的那一轮删掉」。

### 16.4 检查过、确认不是问题的

- **孤儿日志文件**：tick 进了归档，`.zloop/log/20260830-*-t1-done.md` 留在原地。
  `doctor` 的 `missing_log` 查的是反方向（tick 指着不存在的文件），这边不报。
  但文件还读得到、`zloop doc` 少的那几轮本来就随 tick 走了，没有任何路径会崩——记一笔，不算缺陷。
- **NOTES.md 的经验/约定**：`notes.rs` 存的是 `- <RFC3339> 正文`，没有 todo id 这个概念，
  compact 也从不碰这个文件。t40 问的第三个受害者**不存在**。
- **`ensure_idle` 的 TOCTOU**：它在锁外 `state::load`，`state::transaction` 才拿锁，
  中间插一次 `zloop next` 确实能让 `in_progress` 在检查之后才出现。但赢了这个竞态也没用：
  `next` 派出去的那条状态是 `open`，而 `old_ids` 只收 `is_terminal` 的。
  直接把竞态的结果摆出来试（`zloop next` 之后 `compact --keep-days 0 --force`）：
  `nothing to compact`、`t1` 还在清单里、`doctor` 没发现问题。**没复现。**

## 17. 第十四轮：`compact` 剩下的两处指针一起收口（T40-①/② 已修）

T39 修的是三处 todo 指针里的一处（`blocked_by`）。这一轮把剩下两处
（`Tick.todo`、`InProgress.todo`）补齐——形状和 T39 完全一样：**归档里的东西捡不回来，
所以判断力要用在搬走之前，不是搬走之后说一声。**

### 17.1 `cmd_compact` 现在是「先挑到期的，再四道闸逐个往外挑」

```
到期的（终态 + 自己的时间戳早于 cutoff）
  ① in_progress.todo        → 在飞的那一轮，--force 也不搬        （T40-②）
  ② pending_feedback 指着的 → 人还没读到的话，一个字都不许动      （T40-①）
  ③ 名下最新一条 tick ≥ cutoff → 最近还有动静，不算老账            （T40-① 的一般形）
  ④ dead_if_removed         → 还有没做完的 todo 在等它            （T39）
剩下的才真的搬
```

每挑走一条都记下**为什么**（`struct Compacted` 的四个字段），因为四种"留下"的出口
完全不一样：等它的那几条做完 / 人读到那句话 / 在飞的那一轮写回 / 干脆再等几天。
合成一句「留下 N 条」等于让人不知道该敲哪条命令。

`④` 排在最后一道是有意的：前三道挑走的那些本来就不搬，拿**真正的搬运名单**去问
「搬走之后谁会死」才准（原来是拿全部到期的去问，会把已经留下的那条也报成"被等着"）。

### 17.2 T40-① 为什么要两道闸（②③ 缺一不可）

| 场景 | ② 待读反馈 | ③ 名下最近有记录 |
|---|---|---|
| 今天留的话，挂在 40 天前完成的 todo 上 | 拦住 | 也拦住 |
| **35 天前**留的话，循环停着一直没人读 | **拦住** | 放行（反馈自己也过期了） |
| 今天有人 `zloop edit` 改了一条老 todo 的文字 | 放行（不是反馈） | **拦住** |

③ 的写法是「**一条 todo 的年龄看它名下最新的那条记录**」，不是「把那条新 tick 单独留下」。
后者会当场造出第二种悬空指针（`tick.todo` 指着归档里的 id），而前者顺带钉死了一条不变量：
**todo 和它的 tick 永远一起走**。

### 17.3 T40-② 的出口只印真的能用的那条

`ensure_idle` 印的是「先 `zloop done t1` 收尾（或 `zloop edit t1 --status open` 放回去）」。
但 ① 拦下的这条 todo **必然已经了结**（`old_ids` 只收终态），而 `zloop done` 对终态的 todo
退 2 `done: t1 is already deferred`——**这两条出口在这个状态下只有后一条走得通**（实测）。
所以 `compact` 的回显只给能用的那条：

```
nothing compacted：到期的 1 条都留下了，还在清单里
  ⏸ 留下 t1 没搬：还有一轮正拿着它没写回（--force 也不搬）
  ↳ 先让那一轮收尾：zloop edit t1 --status open 放回去，再 zloop done t1 …
```

（`ensure_idle` / `status` 那两处也印着同一条走不通的 `zloop done`，那是**另一处**缺陷，
入口不同、影响面更大，单独排一条，见下一轮 todo。）

### 17.4 回归测试

| 测试 | 钉住什么 | 撤掉后 |
|---|---|---|
| `cli_test::compact_keeps_feedback_nobody_has_read_yet` | ① 今天的话整理后 `context` 里还在、点名 `t1 ← 1 条反馈还没人读`、todo 也留着所以 `edit t1 --status open` 还能用；② 老的未读反馈同样不搬、`nothing compacted` 不说成 `nothing to compact`；读到之后（下一轮 done 落地）照常搬走；③ 今天 `edit` 过的老 todo 留下、两条 tick 都还在 | 删掉 ②③ 两处 `old_ids.remove` → `panicked … 人还没读到的话被整理走了：compacted 1 todos and 2 ticks`（而回显同时印着「留下 1 条没搬」——**闸和回显必须是同一个判断**）；只删 ② → `老的未读反馈同样不许搬`；只删 ③ → `todo 和它的 tick 永远一起走` |
| `cli_test::compact_leaves_the_round_that_is_still_in_flight` | `--force` 之后 `doctor` 仍退 0；点名 `留下 t1 没搬` + `--force 也不搬`；给的出口 `edit t1 --status open` 真的能用，接着 `done t1` 退 0（修复前退 2 unknown todo id）；那一轮写回之后照常搬走 | 删掉 `out.in_flight = …filter(|id| old_ids.remove(id))` 那一行 → `panicked … 整理留下了 doctor 认定的坏状态：compacted 1 todos and 1 ticks`（doctor 退 1） |

`cargo test` 187 全过（原 185）。三处老的 compact 测试一个字没改——它们用的场景里
四道闸都不响（backdate 时把 tick 一起改老了、`--keep-days 0` 下秒级截断的时间戳严格早于
`cutoff`），说明新增的闸没有顺手改掉正常整理的行为。

## 18. 第十五轮：T42（中高）派活指着一条已了结的 todo 时，四处出口一起坏 — 已修

§17.3 那句「入口不同、影响面更大，单独排一条」就是这一条。

### 18.1 造这个状态不需要 `--force`，也不需要手改文件

两步都在最普通的用法里：`zloop next` 派出 t1，人看了一眼判它不做了 →
`zloop edit t1 --status deferred`。`cmd_edit` 从头到尾不碰 `in_progress`（`cli.rs:1037-1111`），
于是派活指针留在一条 `deferred` 的 todo 上。**这是「派出去之后人改主意」，不是异常路径**——
T40-② 需要 `--force` 才走得到，这一条一道闸都不响。

### 18.2 四处出口同时失效（实测，修复前）

| 出口 | 印的 / 做的 | 实际 |
|---|---|---|
| `zloop done t1 …` | `status` 的「写回」那一行、`next --json` 的 `writeback`、runner 每轮塞给模型的写回指令 | 💥 exit 2 `done: t1 is already deferred` |
| `goals::ensure_idle`（`compact` / `goal new` / `goal switch` 共用） | 「有会话正拿着 t1 第 1 轮还没写回：先 `zloop done t1` 收尾」 | 指的正是上面那条退 2 的命令 |
| `zloop status` | 同一屏上「清单 t1 ⏭ 已延后」+「阶段 claude 正在做 t1 · 第 1 轮」 | 两块屏对同一份 state 说相反的话 |
| `zloop doctor` | — | 一声不吭 exit 0「没发现问题」 |

最狠的是第一条落在**无头轮次**上：runner 每轮的收尾指令写死是「本轮结束前必须执行写回命令
`zloop done <id> …`」，模型手里只有这一条命令，而它保证失败。第四条是老熟人——A-7 同一个形状：
三条命令被拦下，唯一负责回答「哪儿不对」的那个没话说。

### 18.3 修法：让 `done` 收得了尾，且**不改状态**

两个候选：把出口全改成 `edit --status open`，或者让 `done` 在这个状态下能收尾。选后者，
因为前者是**劝人撤销自己刚做的决定**（把 deferred 改回 open 再 done），而且要在三处分别
判一次状态。

`tick::apply_done` 的终态闸从「一律拒绝」收窄成「**`in_progress` 不指着它才拒绝**」：

```rust
let settled = todo::is_terminal(&status) && state.in_progress.as_ref().is_some_and(|ip| ip.todo == id);
if todo::is_terminal(&status) && !settled { bail!("{id} is already {status}"); }
```

`settled` 为真时四个分支（block / done / progress / fail）**全部跳过对 todo 的写**——
状态、note、`blocked_by` 一个字都不动，只 `record` 这一轮的 tick，再由 `cmd_done` 清掉
`in_progress`。把 `deferred` 写成 `done` 才是 §16.3 说的那条「假的完成」：人判过的东西
不该被一次机械的写回覆盖回去。

「为什么状态没动」是**结构化字段**不是回显自己判的：`apply_done` 返回
`Written { tick, idx, kept_status }`，`kept_status: Option<String>` 就是那条终态；
`cmd_done` 只渲染它。（§17 的教训：判断和回显分成两处写，回显迟早替一个不存在的行为背书。）

配套的三处：

- `ensure_idle` 点出状态并只给一条出口：`t1 已经 deferred 了，但第 1 轮的派活还挂在它上面：
  先 zloop done t1 … 收尾（状态不会被改回去），或加 --force`。不再劝 `--status open`。
- `phase::compute` 在 `executing` 这一行后面接一句 `⚠ 已经延后了，写回只清派活`
  （`summary` 里是 `⚠ already deferred; write-back only clears the hand-out`）。
  改在 `phase` 而不是 `status`：`status` / `context` / `next --json` 读的是同一个函数。
- `doctor` 新增 `settled_in_progress`（warn）：`第 1 轮派出去的 t1 已经是 deferred 了，
  派活指针还挂在它上面` → 修法给的是现在真能敲的那条 `zloop done t1 …`。
  判 warn 不判 err：一条正常的 `done` 就收得掉，循环也照跑（下一次 `next` 会重新派活、
  顺手盖掉这个指针）。
- `compact` 的 in-flight 提示（§17.3 那个两步走）收敛成一条命令。

### 18.4 闸只对在飞的那条开

`in_progress` 不指着它时一切照旧退 2 —— 三种情形实测：写回之后再 `done` 一次
（`done` 自己把指针清了）、在飞的是 t2 却去 `done t1`、从没派过活的 todo。
`done_twice_is_rejected`（`tick_test`）和 `done_errors`（`cli_test`）一个字没改，全过。

### 18.5 回归测试

| 测试 | 钉住什么 | 撤掉后 |
|---|---|---|
| `cli_test::writing_back_a_round_whose_todo_was_already_settled` | ① `doctor --json` 里有 `settled_in_progress` 且 `fix` 给的是 `zloop done t1`；② `status` 同屏上「已延后」和「正在做 t1」被一句话接上；③ `ensure_idle` 点名 `t1 已经 deferred 了`、不再劝 `--status open`；④ `done` 退 0、印「状态没动」、`in_progress` 清掉、**t1 仍是 deferred**、tick 照记；收尾后 doctor 干净、闸放行 | 把 `settled` 写死成 `false` → `panicked … 写回得收得了尾：done: t1 is already deferred / left: 2 right: 0` |
| `cli_test::done_twice_is_still_rejected_when_nothing_is_in_flight` | 闸只对在飞的那条开：写回之后再 `done`、在飞的是 t2 时 `done t1`，都退 2 `already done` | 把 `settled` 放宽成 `is_terminal(&status)` → 两处都退 0，重复写回 |

`cargo test` 189 全过（原 187）。`compact_leaves_the_round_that_is_still_in_flight`
改了两行——它原来断言的出口是 `zloop edit t1 --status open`（当时 `done` 走不通），
现在断言 `zloop done t1` 并直接敲它。

---

## 19. 第十六轮：T21（中）`awake.rs` 的 8 处裸子进程 — 收口 5 处、留 3 处并写明理由

t20 收完 git 之后留了一句「**每个 zloop 起的子进程都走 `run_capture`**」，但 `awake.rs`
里还有 8 处裸的。这一轮的题目是**评估要不要收口**，结论是「5 处要、3 处不能」，
而且「不能」的那 3 处不是懒得改——真改了会把功能弄坏。

### 19.1 先把 8 处列全（`grep -n "Command::new" src/`，一处不漏）

| # | 位置 | 命令 | 谁在等它 | 结论 |
|---|---|---|---|---|
| 1 | `sleep_disabled()` | `pmset -g` | runner 启停 + `status` + `awake` | **收口** |
| 2 | `sudo_ok()` | `sudo -n pmset -g` | 同上 | **收口** |
| 3 | `set_sleep_disabled()` | `sudo -n pmset -a disablesleep 0/1` | 同上 | **收口** |
| 4 | `on_battery()` | `pmset -g batt` | `status` 那行电池警告 | **收口** |
| 5 | `install_sudoers()` | `visudo -c -f <tmp>` | 人敲 `zloop install --sudoers` | **收口** |
| 6 | `spawn_caffeinate()` | `caffeinate -i -s -w <pid>` | **没人等** | 保留 |
| 7 | `spawn_watchdog()` | `sh -c 'while kill -0 …'` | **没人等** | 保留 |
| 8 | `install_sudoers()` | `sudo install -o root … /etc/sudoers.d/…` | 人（要打密码） | 保留 |

顺带把剩下两处也核了：`runner::preflight` 走 `run_with_timeout` → `run_capture`（早就带闸），
`daemon::start` 起的是 detach 出去的 runner 本身（和 6/7 同一类）。至此 `src/` 里
**没有第 9 处**。

### 19.2 1–4 不是「不在热路径上」，它们全在**收尾路径**上

排 t21 时写的是「不在热路径、也不是会挂住的那一类」。这句话错了一半：

`stop()` 的**第一行**就是 `awake::release()` → `reconcile()` → 1、2、3 三条命令。
和通知那一下（`notify.rs` 的注释已经写明）是同一种死法：这里挂住，runner 就
**不记 `stop`、不清 pid 文件、退不出去**——「干完就停」卡在最后一米，而且
`AwakeGuard::drop` 里还有第二遍。

会不会挂住？`pmset` 要跟 `powerd` 说话；`sudo` 要过 sudoers 解析 + 目录服务
（公司 Mac 绑了 AD/LDAP 时 `sudo` 等的是网络）。裸 `.output()` / `.status()` 对这两种
stall 都是无限期等待。

**实测（修复前的等价实现）**：PATH 上放一个 `pmset` = `sleep 600`，
`zloop awake reconcile` **一个字都不印，30 秒后被测试 SIGKILL**：

```
thread 'hung_pmset_cannot_wedge_the_awake_probes' panicked at tests/runner_test.rs:85:5:
`zloop awake reconcile` 过了 30s 还没自己退出
--- stdout ---
--- stderr ---
```

修复后同一个场景：1 秒到点整组收掉，`awake: `pmset -g` 超过 1s 没回来，已整组收掉；
这次当读不出来`，然后正常退 0 并印 `unknown (pmset -g unreadable)` / `unchanged`——
**读不出当前值时一条写命令都不发**（`pm_log` 为空）。

### 19.3 这一轮真正的发现：无脑套 `run_capture` 会把恢复默认值弄没

`run_capture` 原来有**两个**闸：超时，和 `stop_requested()`。第二个对干活的子进程是对的，
对**收尾**的子进程是致命的——`zloop stop` 的 SIGTERM 先把标志置上，之后才走到
`stop()` → `awake::release()`。那三条 `pmset` 探针要是也认叫停，会在 `run_capture`
第一次轮询里被**自己**杀掉：`sleep_disabled()` 读不出来 → `(None, _)` 落到
`_ => None` → `set_sleep_disabled` 根本不调用 → `SleepDisabled=1` 原地留着。
装了闸，反而把功能弄没了。

所以 `run_capture` 多了一个显式的 `Stop` 参数，和 `Group` 一样，逼每个调用方
当场表态：

| | 语义 | 谁用 |
|---|---|---|
| `Stop::Honor` | 超时 **+** `zloop stop` 都收 | 宿主、preflight、git 检查点、`log::changed_files`、通知 |
| `Stop::Ignore` | **只**认超时 | `awake` 的 5 条（它们是在替我们收拾现场） |

`Group` 这里挑 `Own` 也是同一个道理：被外面整组 `killpg` 时，「把设置改回去」这条
命令必须能从那一刀底下活下来把活干完。

**实测（把 `Ignore` 改成 `Honor`）**：runner 被 `zloop stop` 之后，账本里
**根本没有 `awake_off` 这一条**——

```
[…{"event":"end",…,"interrupted":true,…}, {"event":"stop","reason":"sigterm",…}]
disablesleep 1
disablesleep 0
```

注意 `pm_log` 末尾那个 `disablesleep 0`：**是一秒后看门狗补的，不是 runner 自己干的**。
所以这条回归测试的断言挑的是**账本**（`awake_off` + `restored_default:true`）而不是
最终值——只看最终值，看门狗会把这个 bug 全程盖住，测试永远绿。

### 19.4 6/7/8 为什么**不能**收口（写进代码注释，不只写在这里）

- **6 `caffeinate` / 7 看门狗**：它俩的全部职责就是**比 runner 活得久**（`-w <pid>`；
  `kill -9` 之后替我们恢复默认值）。`run_capture` 是「等它退出」，在这儿等 = 等到长跑结束 /
  等到它已经没用了。它们也不需要闸——我们从不等它们，它们挂不住我们。
- **8 `sudo install`**：这一下**在终端上问密码**。`run_capture` 把 stdin 接 `/dev/null`、
  stdout/stderr 接管道，密码提示到不了人眼前、人打的字也进不去，装上闸等于把
  `zloop install --sudoers` 弄坏；「人在键盘前想多久」也没有合理的超时可定。
  它只在人手敲那一条命令时跑，不在 runner 的任何路径上。

于是那句总结改成一句**真的**话，写在 `run_capture` 的文档注释里：
**每一个 zloop 起的、要等它退出的子进程都走 `run_capture`；不等的（detach 出去、
故意活得更久的）和要人手打字的，在原地写明为什么。**

### 19.5 回归测试

| 测试 | 钉住什么 | 撤掉后 |
|---|---|---|
| `runner_test::hung_pmset_cannot_wedge_the_awake_probes` | `pmset` = `sleep 600` 时 `zloop awake reconcile` 30 秒内自己退 0，stderr 说出超时，且**不发任何写命令** | 把 `power_cmd` 换回裸 `.output()` → 测试**挂住**，30s 后 SIGKILL + panic（用 `run_within` 兜底正是为此：挂住的测试没人当成失败） |
| `runner_test::sigterm_still_lets_the_runner_restore_the_sleep_default` | `zloop stop` 之后 runner **自己**把 `disablesleep` 改回去：账本里有 `awake_off` + `restored_default:true` | `Stop::Ignore` → `Stop::Honor` → 账本里没有 `awake_off`（值最后还是 0，但那是看门狗补的） |

`cargo test` 191 全过（原 189）。`cargo fmt --check` 干净；`cargo clippy --all-targets`
的 4 条 warning 和 HEAD 逐条相同，没新增。

### 19.6 顺手记下、没在这一轮动的

`install_sudoers()` 把规则先写到 `env::temp_dir()/zloop-pmset.<pid>`（0644），再
`sudo install` 到 `/etc/sudoers.d/`。写和 install 之间有一个窗口，落到 `/tmp` 时
（`TMPDIR` 没设）文件名是可预测的。macOS 上 `TMPDIR` 默认是每用户 0700 的
`/var/folders/…`，所以实际可利用性低——但这是提权面上的 TOCTOU，值得单独排一条查清楚，
不该混在这一轮的子进程收口里。

→ 已在 §20（T43）查完并修掉。**上一段括号里那半句「`TMPDIR` 没设」是错的**，真正的触发条件
见 §20.1。

## 20. 第十七轮：T43 —— §19.6 那条尾巴查到底

### T43（中）`install_sudoers` 的暂存路径别人也能占名，装进 `/etc/sudoers.d/` 的可以不是我们写的那份 — 已修

修之前的三步（`src/awake.rs`，HEAD~1 的 306–330 行）：

```rust
let tmp = env::temp_dir().join(format!("zloop-pmset.{}", process::id()));  // 名字可猜
fs::write(&tmp, &rule)?;                                                   // 不 O_EXCL、不 O_NOFOLLOW
visudo -c -f $tmp                                                          // 语法检查
sudo install -o root -g wheel -m 0440 $tmp /etc/sudoers.d/zloop-pmset      // 从**同一条路径**重新读一遍
```

`fs::write` 和 `sudo install` 是对同一条路径的**两次独立解析**，中间那段时间里这条路径指向什么，
装进 `/etc/sudoers.d/` 的就是什么。而这条路径的名字是 `pid`，猜得到。

### 20.1 先纠正 §19.6 里写错的那半句前提

§19.6 写的是「落到 `/tmp` 时（`TMPDIR` 没设）」。**这半句是错的**，实测（rustc 1.98，macOS 26.5）：

```
$ env -i ./probe                 # 环境里一个变量都没有
temp_dir = /var/folders/ym/md7fzwnn27961mm7xh48mmgc0000gn/T/
$ env -i ./confstr_probe
confstr n=50 val=/var/folders/ym/md7fzwnn27961mm7xh48mmgc0000gn/T/
$ TMPDIR=/tmp ./probe
temp_dir = /tmp
```

Rust 在 Apple 平台上的 `env::temp_dir()` 不是「`TMPDIR` 没设就 `/tmp`」——没设时它走
`confstr(_CS_DARWIN_USER_TEMP_DIR)`，拿到的仍然是那个每用户 0700 的 `/var/folders/…/T/`
（`drwx------ zouhuigang staff`）。所以**默认的 mac 落不到共享目录里**，别的 uid 连那个目录都进不去。

要落到共享目录，得有人**显式**把 `TMPDIR` 指过去——`export TMPDIR=/tmp` 是绕开
`/var/folders` 长路径（Unix socket 107 字节上限之类）的常见手工设置，`/tmp` 是 `drwxrwxrwt`。

于是完整前提是两条，**缺一不可**，这也是它定 P2 而不是 P0 的原因：

1. `TMPDIR` 指向一个别的 uid 也写得进的目录；
2. 机器上有第二个非 root uid 在跑代码（另一个登录用户，或被拿下的 `_www` / `nobody` 之类服务账号）。

### 20.2 前提成立时，两种占名方式都成（实测）

`sh scripts/repro-t43-sudoers-tmp-swap.sh` 的第一部分照抄修之前那三行跑：

```
=== 修之前 · 占名方式 A：软链接指到攻击者的文件 ===
  源路径 = /tmp/zloop-pmset.4145   ← temp_dir + pid，猜得到
  fs::write 之后 mode = 0644（属主/权限位都还是他挑的）
  真正落地的实体 = /private/tmp/zloop-t43.XFt9JY/attacker/payload
  install 退出码 = Some(0)
  → 装进 sudoers.d 的是：
      # zloop: 攻击者的规则
      attacker ALL=(root) NOPASSWD: ALL

=== 修之前 · 占名方式 B：攻击者自己的 0666 普通文件（连软链接都不用） ===
  fs::write 之后 mode = 0666（属主/权限位都还是他挑的）
  → 装进 sudoers.d 的是：
      attacker ALL=(root) NOPASSWD: ALL
```

两种都能走通，因为**代码对属主一个字都没检查**：

* A：`fs::write` = `File::create`，没有 `O_NOFOLLOW`，顺着软链接写进他的文件；随后 `install`
  再顺一次，读到的是他那一刻放进去的内容。
* B：`File::create` 撞上已存在的文件不是失败而是 `O_TRUNC` 接着写——**属主和权限位都不动**。
  所以那句「文件是 0644」也是乐观的：权限位是占名的人挑的（实测 0666），我们写完他照样改得动。

`/tmp` 的 sticky 位（`t`）挡的是「删掉/改名别人的文件」，挡不住「先占住一个还不存在的名字」。
pid 也不是屏障：macOS pid 顺序递增、`ps` 看得见，提前把候选名字铺满是几十 KB 软链接的事。

### 20.3 两道看起来像闸的东西，都不拦这个

* **`visudo -c`**：它检查的是语法，攻击者的规则语法完全合法（实测 `parsed OK`）；何况它跑在
  窗口**之前**，换内容发生在它之后。
* **那次密码提示**：`sudo install` 会停下来等人打密码——这不是保护，这是把窗口**拉长**到
  「人在键盘前想多久」。窗口从来不是「write 到 install 那一瞬」。

### 20.4 修法：换掉的不是名字，是**父目录**

只把名字改随机是不够的（那只解决「猜得到」，没解决「那个目录别人写得进」）。现在的
`awake::stage_rule_in(base, rule)`：

* `mkdir` 一个随机名的 **0700** 目录。`mkdir` 是原子的：名字被占（哪怕被占成一条软链接）就是
  `EEXIST`，我们换个名字重来，**绝不会悄悄接手别人的目录**；
* 规则文件用 `create_new`（`O_EXCL`）+ **0600** 建在这个目录里面。父目录是我们自己刚建的，
  别人进不去，也就没有「占名」这回事了——安全边界不再取决于 `TMPDIR` 指向哪儿。
* umask 只会再削权限位、不会加，所以拿到的东西不可能比 0700/0600 更松。
* 随机名只是让「预先把候选名字铺满」这种拒绝服务也不成立，**它不是边界本身**。

顺手补掉的一处漏：清场从「`sudo install` 之后删一次」改成 `StagedRule` 的 `Drop`。修之前
`visudo` 拒绝那一支是直接 `bail!` 的，临时文件留在原地没人管。

### 20.5 回归测试

| 测试 | 钉住什么 | 撤掉修复后 |
|---|---|---|
| `runner_test::the_sudoers_rule_is_staged_out_of_reach_of_other_users` | 在一个 0777 的 base 里：① 预先按老名字 `zloop-pmset.<pid>` 占的位一个字都没动 ② 同进程两次暂存路径不同（名字不可猜）③ 父目录 0700 且是新建的（只有我们那一个文件）④ 文件 0600 ⑤ `Drop` 之后目录和文件都没了 | 把函数体换回老写法 → 第一条就红：<br>`assertion left == right failed: 别人占住的名字不该被我们接手`<br>`  left: "# zloop: let the runner keep the Mac awake…"`<br>` right: "# 攻击者占的位\n"` |
| `scripts/repro-t43-sudoers-tmp-swap.sh` | 第二部分把同一手占名打在**仓库里现在这个** `stage_rule_in` 上（链 `target/debug/libzloop.rlib`，不是抄一份） | 退出码 0 → 1，并印出 `[FAIL] 回归了：别人占住名字之后，stage_rule_in 又跟着写了` |

`cargo test` 192 全过（原 191）。`cargo fmt --check` 干净；`cargo clippy --all-targets`
的 4 条 warning 和 HEAD 逐条相同（`awake.rs:9` / `notify.rs:8,9` 的 doc 缩进、`tick.rs:293`
的 lifetime），没新增。

### 20.6 顺手确认过的

* `grep -rn "temp_dir" src/` 全仓库只有这一处，没有第二个同型的暂存点。
* `visudo -c -f` 读得动新暂存的文件（0600、在 0700 目录里、默认 `TMPDIR`），实测：

  ```
  暂存 = /var/folders/ym/…/T/zloop-sudoers.f5d7cb9f3a1b5de9/zloop-pmset
  visudo -c → …/zloop-pmset: parsed OK   退出码 = Some(0)
  ```

### 20.7 没验的那一步（说清楚，别当验过）

最后那下 `sudo install -o root -g wheel -m 0440 <暂存> /etc/sudoers.d/zloop-pmset` **没有在本机跑**：
它要么弹密码、要么就真的动这台机器的 `/etc/sudoers.d/`，都不是审查该做的事。这一步涉及的
只有「root 去读一个属主是本人的 0600 文件」——root 不受权限位约束，且**修前修后它读的都是
同一个临时目录底下的文件**，这一层没有变化。真正变了的是那条路径别人还占不占得住名字。

## 21. 第十八轮：T29 —— `compact` 搬走的不只是花费，还有「这个目标跑了多久」

### T29（中）一次例行整理，把 `status` / `stats` / `replan` / 轮次编号四处读数一起清零 — 已修

A-18（§见前）修的是**花费**：`compact` 把老 todo 名下的 tick 搬进 `archive/`，而钱记在
tick 上，于是预算闸被静默复位。当时的修法是给 `Archived` 加一个 `cost_usd` 累计，
`tick::spent_total` 把它加回来。

问题在于：**花费只是第一个被搬走的累计量，不是唯一一个**。`state.ticks` 同时还是
「跑了几轮」「返工几轮」「失败几次」「无文档几轮」「宿主累计多久」「现在是第几轮」
的唯一数据源，而这些数全是**现算**的。搬走 tick，它们一起掉下去。

### 21.1 复现（`sh scripts/repro-t29-compact-drops-round-count.sh`，修之前退 1）

一个跑了 4 轮（done / progress / fail / done）、完成过 2 条 todo、返工率 50% 的目标，
把前两条 todo 和它们的 tick 做旧到一个月前，然后跑一次**默认参数之外什么都没做**的整理：

```
=== 整理之前 ===
  status    ▶  就绪      ██████████░░░░░░ 66%  跑了 4 轮
  next    round = 3
  stats   轮次    4 轮 · 返工 2（50%）· 失败 1
  replan  [rework] 返工率 50%（最费劲的是 t2）

=== 人做了一次例行整理：zloop compact --keep-days 30 ===
  compacted 2 todos and 4 ticks → …/.zloop/archive/compact-….json

=== 整理之后 ===
  status    ▶  就绪      ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  next    round = 0
  stats   还没有跑过任何一轮 · zloop next 开始
  replan  （没有返工信号：重估不会被触发了）
```

四处读数，坏法各不相同：

| 读数 | 坏成什么样 | 为什么这不只是好看不好看 |
|---|---|---|
| `status`「跑了 N 轮」 | 4 → 0 | 交接包和 `status` 是人判断「这轮循环干了多少」的唯一入口 |
| `stats` | **整页消失**：`rounds == 0` 时它印一句「还没有跑过任何一轮 · zloop next 开始」就 `return` | 跑了 4 轮、完成过 2 条的目标被劝去「开始跑第一轮」 |
| `replan` 的 rework 信号 | 熄火（阈值是 `rounds >= 3 && rate >= 0.5`） | 这是**自动重估**的触发条件之一，不是显示——一次整理等于把这条闸拆了 |
| 轮次**编号**（`tick.round` / 交接包的 `round N`） | 3 → 0，下一条 tick 又从 1 开始 | 编号不是余额，它只该增：归档里已经有 `round 1`，现在账本里又有一条 |

`stats` 那一处最值得单说：它不是数字小了，是**整页不见了**——`rounds == 0` 的早退分支
本来是给「刚 init 完还没跑」准备的，而 compact 把一个跑了很久的目标伪装成了那个状态。
「一轮都没跑过」和「跑过的都被整理走了」是两回事，只有前者该劝人去 `zloop next`。

### 21.2 修法：记的不是「几轮」，是**按 outcome 分的计数**

顺着 A-18 再加一个 `archived.rounds` 是能把这一条摁下去的，但那是在等下一个字段被发现。
根因是「`compact` 搬走了 ticks，而所有累计量都从 ticks 现算」，所以归档汇总里存的应该是
**能重算出任何一个累计量的那份原料**：

```rust
pub struct Archived {
    pub ticks: usize,                              // 总条数（A-18 就有）
    pub outcomes: BTreeMap<String, usize>,         // 新：按 outcome 分的条数
    pub undocumented: usize,                       // 新
    pub duration_ms: u64,                          // 新
    pub cost_usd: f64,                             // A-18
    pub at: Option<String>,
}
```

`Archived::rounds()` 从 `outcomes` 里按 `tick::COUNTED`（done/progress/fail）求和——
和 `tick::rounds` **共用同一个定义**，不再手写第二遍。读的一侧多两个函数：

* `tick::rounds_total(state)` = `rounds(&state.ticks) + archived.rounds()`，`status` 和
  `stats` 共用（和 `spent_total` 是同一件事的两个面）；
* `tick::round_number(state)` = `current_round(&state.ticks) + 归档里的 done+progress`，
  盖在新 tick 上、印在交接包里的那个编号，5 处调用一起换过去。

`stats::compute` 里那个 `counted(o)` 闭包一处加上 `+ archived.count(o)`，
轮次/返工/失败/被挡/反馈/回看/重估七个数一起补齐。**返工率的分子和分母必须同源**——
只补分母（或只补分子）会让一次整理把返工率冲歪，而 `replan` 拿这个数当信号。

### 21.3 口径：哪些数是「一辈子」，哪些只是「账本里还剩的」

修完之后 `stats` 的表头和它下面那张 todo 清单**必然对不上**（40 轮 vs 清单里 2 条），
因为归档走的 todo 连 id 都不在了。这不是 bug，是 compact 的本意——但不说出来，
下一个人只能当它是 bug。所以 `stats` 多一行、`reflect` 的材料包多一句：

```
  归档    上面含整理走的 4 轮 · 4 条记录在 .zloop/archive/（清单只列账本里的）
```

**老状态文件**（T29 之前的 compact 只记了 `ticks` 和 `cost_usd`）那些轮次是真的补不回来了。
这时 `Archived::rounds_unknown()` 为真，`stats` 说的是「老版本整理走 N 条记录，轮次没记」——
不知道就说不知道，**不许把「不知道」印成「0 轮」**，更不许接着劝人去 `zloop next`。

### 21.4 没修的那一半（说清楚，别当没看见）

同一次整理还会让两个**从 todo 现算**的数掉下去，这一轮没动：

* `status` 的进度条和百分比：66% → **0%**（`finished/planned` 数的是 `state.todos`）；
* `stats` 的「一次过 X/Y 条」：Y 是 done 的 todo 数，整理走的那些不在了。

它们和轮次不是同一个判断：清单缩短本来就是 compact 的目的，「还剩几步」理应只算还剩的；
可百分比又确实在回答「这个目标做到哪儿了」。要不要把归档的 todo 数也算进分母，
是个口径决定（还牵扯清单里「步骤 1..N」的编号），单独排一条，别混在这一轮里。

> **已在 §22（T44）做掉**：口径定成「做到哪儿了」含归档、「还剩什么」不含，
> 步骤号钉死为「剩下这张清单里的执行顺序」。那一轮还数出第三个出口（`goals` 的进度列）。

### 21.5 回归测试

| 测试 | 钉住什么 | 撤掉修复后 |
|---|---|---|
| `cli_test::compact_does_not_reset_how_many_rounds_this_goal_has_run` | 整理前后 `status`「跑了 4 轮」、`stats`「4 轮 · 返工 2（50%）· 失败 1」「无文档 2 轮」、`replan` 的「返工率 50%」全都不变；`stats --json` 的 rounds/rework/fails/rework_rate/archived_rounds；再整理一次不重复计数；老版本汇总走 `rounds_unknown` 那一支 | 把 `rounds_total` / `counted` 的归档项去掉 → `整理之后 status 的轮数掉了：… 0%  跑了 0 轮` |
| `cli_test::compact_does_not_hand_out_a_round_number_twice` | 整理之后 `next --json` 的 `round` 不掉、`in_progress.round` 接着数、新 tick 的编号不撞归档里用过的 | 把 `round_number` 换回 `current_round` → `整理之后编号掉回去了：{… "round":0 … "phase":"executing t3 · round 1 …"}` |
| `scripts/repro-t29-compact-drops-round-count.sh` | 四处读数一起看（status / next 的 round / stats / replan 信号） | 退出码 0 → 1，并印出上面 §21.1 那段「整理之后」 |

`cargo test` 194 全过（原 192）。`cargo fmt --check` 干净；`cargo clippy --all-targets`
的 4 条 warning 和 HEAD 逐条相同（`awake.rs` / `notify.rs` 的 doc 缩进、`tick.rs` 的 lifetime），
没新增。

---

## 22. 第十九轮：T44 —— 「这个目标做到哪儿了」也是 `compact` 搬得走的

### T44（中）整理一次账本，进度条 66% → 0%、「一次过 2/2」→「0/0」 — 已修

T29 修的是从 **ticks** 现算的那一族（轮次 / 返工 / 失败 / 无文档 / 耗时 / 轮次编号），
当时明写了「没修的那一半」（§21.4）：从 **todo** 现算的还有两个。这一轮把它们做掉，
顺带发现第三个出口。

### 22.1 复现（`sh scripts/repro-t44-compact-drops-progress-percent.sh`，修之前退 1）

三条待办、完成两条（进度 2/3），把它们做旧到一个月前，跑一次默认参数的整理：

```
=== 整理之前 ===
  status    ▶  就绪      ██████████░░░░░░ 66%  跑了 2 轮
  stats   质量    一次过 2/2 条 · 无文档 2 轮 · 被挡 0 次 · 用户反馈 0 条
  goals   ▸ t44-repro  进行中  2/3  … t44 repro

=== 人做了一次例行整理：zloop compact --keep-days 30 ===
  compacted 2 todos and 2 ticks → …/.zloop/archive/compact-….json

=== 整理之后 ===
  status    ▶  就绪      ░░░░░░░░░░░░░░░░ 0%  跑了 2 轮
  stats   质量    一次过 0/0 条 · 无文档 2 轮 · 被挡 0 次 · 用户反馈 0 条
  goals   ▸ t44-repro  进行中  0/1  … t44 repro
```

| 读数 | 坏成什么样 | 为什么这不只是好看不好看 |
|---|---|---|
| `status` 的进度条 / 百分比 | 66% → **0%** | 同一行左边写着「跑了 2 轮」（T29 修好了，一辈子的账），右边说做到 0%——**一行两个口径**，而且没有任何标记 |
| `stats` 的「一次过 X/Y 条」 | 2/2 → **0/0** | 同一张表上面写着「2 轮」，质量行说一条都没完成过；`0/0` 还会让 `pct()` 印出 `—` |
| `goals` 的进度列 | 2/3 → **0/1**（§21.4 没数到的第三处） | 多目标那张表是「哪个目标做到哪儿了」的唯一入口 |

分子分母**一起**掉是这一条和 T29 的区别：不是数字变小，是比例被重置成 0%——
一个跑了两轮、完成两条的目标，在三个出口上同时显示成「没开始」。

### 22.2 口径（这一条 todo 要定的就是它）

**「做到哪儿了」的数含归档；「清单里还剩什么」不含。** 一句话把四处都判了：

| 读数 | 口径 | 理由 |
|---|---|---|
| `status` 的百分比 / 进度条 | 含归档 | 它和同一行的「跑了 N 轮」「花了 $X」是同一个问题的三个面，那两个 T29/A-18 已经定成一辈子的账 |
| `stats` 的「一次过 X/Y」 | 含归档 | 它和同一行的「无文档 N 轮」同源，问的是「跑得怎么样」 |
| `goals` 的 done/total | 含归档 | 同上，只是换了个屏 |
| `status` 的清单、`stats` 的 todo 表、`stats::remaining`、「N 条待办全部完成」 | **不含** | 它们回答「还剩什么」，清单变短本来就是 compact 的目的 |
| 清单里的**步骤 1..N** | **不含**，而且不许加偏移 | 归档走的 todo 不一定是清单的前缀：compact 挑的是「已了结且过期」的，中间的照样被挑走。拿一个条数去给步骤号加偏移，只会把「剩下的第 1 步」印成理直气壮的「第 3 步」。步骤号的定义就此钉死：**它是剩下这张清单里的执行顺序**，不是这个目标的第几步 |

两个口径同屏必然对不上（`66%` vs `清单 0/1 完成`），所以**把它印出来**，
和 T29 给 `stats` 加那一行是同一个做法：

```
  清单    0/1 完成 · 归档 2 条
  归档    整理走 2 条待办（完成 2 条），已算进上面的 66%
```

### 22.3 修法：归档汇总里再存一份「todo 那一侧的原料」

沿用 T29 定下的形状——存**能重算出这一族的原料**，不是再加一个计数器：

```rust
pub struct Archived {
    // …… T29 的 ticks / outcomes / undocumented / duration_ms / cost_usd
    pub todos: usize,                            // 新：搬走的 todo 条数
    pub statuses: BTreeMap<String, usize>,       // 新：按 status 分（done / deferred / …）
    pub first_try: usize,                        // 新：其中「一次过」的
}
```

`statuses` 一份原料喂三个读数：`done()` = 分子、`planned()` = `todos - deferred`（延后的
不进分母，和 `status` 里 `planned` 同一口径）。**只有 `first_try` 必须当场算**——
它要的是每条 todo 名下的轮数，而 tick 下一行就不在账本里了；判据抽成
`stats::first_try(status, &mine)`，`compute` 和 `compact` 共用，不许写两遍。

### 22.4 顺带修掉的：整理干净的目标被说成「刚开的」

`status` 那句「◦ 待规划」的分支条件是 `total == 0`，写它时想的是「刚 init 完还没规划」。
可 compact 能把干完活的目标清空成一模一样的样子——**和 T29 里 `stats` 的 `rounds == 0`
早退是同一个坑的第二次出现**：一个为「从没开始」准备的早退分支，被 compact 伪造了前提。
加上 `st.archived.todos == 0` 之后它落回「• 空闲」（和没整理过的全做完目标一致），
阶段行也改说「4 条待办全部整理归档了 · 要接着做就 zloop plan 加几条」。

**老状态文件**（T44 之前的 compact 只搬 todo 不记条数）走 `Archived::todos_unknown()`：
判据是 `at.is_some() && todos == 0`（一次成功的整理至少搬走一条 todo）。这时百分比只算
账本里的，`status` 多一行「老版本整理走过待办，条数没记（没算进上面的百分比）」——
不知道就说不知道，别让百分比替归档里的那些背书。

### 22.5 回归测试

| 测试 | 钉住什么 | 撤掉修复后 |
|---|---|---|
| `cli_test::compact_does_not_reset_how_far_this_goal_has_got` | 整理前后 `status` 的 66%、`stats` 的「一次过 1/2 条」、`goals` 的 2/3 全不变；`archived.todos/done()/planned()/first_try`（返工过的那条不算一次过）；两处归档说明行；`stats --json` 的 done/first_try/archived_todos/archived_done；再整理一次不重复计数；老版本汇总走 `todos_unknown` 那一支 | `tests/cli_test.rs:1074` `整理之后 status 的百分比掉了：… ░░░ 0%  跑了 2 轮` |
| `cli_test::compact_keeps_deferred_out_of_the_denominator_and_does_not_forge_a_fresh_goal` | 延后的 todo 归档后不进分母（66% 不变）；全部整理干净之后不说「待规划」、还是 100%、阶段行说明是整理走的 | `tests/cli_test.rs:1138` `延后的 todo 被算进了归档的分母：` |
| `scripts/repro-t44-compact-drops-progress-percent.sh` | 三处出口一起看（status 百分比 / stats 一次过 / goals 进度） | 退出码 0 → 1，并印出上面 §22.1 那段「整理之后」 |

`cargo test` 196 全过（原 194）。`cargo fmt --check` 干净；`cargo clippy --all-targets`
的 4 条 warning 和 HEAD 逐条相同，没新增。

---

## 缺陷清册搬到了 `docs/FINDINGS.md`

原来这里是「§22 第二十轮（收尾）：全部确认缺陷的 issue 草稿」——42 条确认缺陷的一览表
和逐条草稿。它是**按缺陷查**的入口，却压在三千多行**按轮次读**的正文后面，
两种读法挤在一个文件里，谁都得先滚过对方。t45 把它整节搬到了
[`docs/FINDINGS.md`](../../docs/audit/FINDINGS.md)，内容一字没删，另外做了三件事：

- 每条草稿的「正文 §N」从**一个数字**变成**锚点链接**，点进去落在这边的那一小节上；
- 一览表多一列「正文」，同样是链接；
- 顺带修掉了搬家时暴露出来的三处引用腐烂（重复的节号 6、指错的 §2、一条本来就坏的锚点）。

正文（§1–§22）留在这里。**内容一句没改**，改的只有导航：第四轮及其后的节号各 +1
（原来第三轮和第四轮都叫 6），以及上面说的那三处引用。

---

## §N 的规矩推广到全部文档了

t45 立的那道闸只管两份文档（这一份和 `FINDINGS.md`）——可「靠 §N 指路」是全仓的写法：
`ADAPTIVE-REPLAN` / `LONG-RUN-PROOF` / `loopx-principles` 里都有成片的自指 §N，一份也没在验。
t46 把 R2（节号不重不断）和 R3（§N 指得到）推广到 README + `docs/` 下全部 15 份，
覆盖面从 28 处 §N 变成 **166 处**。

推广要先回答一个原来不用回答的问题：**这个 §N 说的是哪份文档？** 只管一份文档时答案永远是
「本文件」；全仓一起验，跨文档引用就得先归属再查号。归属按三级判：待在链接里 → 按链接指向的
那份；否则看同一行、§ 之前最近提到的 `xxx.md`；都没有才算自指。归到仓库外的文档
（loopx 上游的 `field-derived-patterns.md` 之类，共 4 处）查不了，跳过并计数。

推广当场抓到的：

| 抓到什么 | 为什么前两条规则都拦不住 | 处置 |
|---|---|---|
| `DESIGN.md` 的「notes `§4.3`」 | 指的是 `loopx-scheduling-notes.md` 的 `§4.3`，可 DESIGN **自己也有** `§4.3`（`tick.outcome`）——号**指得到**，只是指到了另一份文档的同号节 | 改写成锚点链接，号和落点一起被验 |
| `DESIGN.md` 的「notes `§3.3`」 | 同一种写法，只是 DESIGN 里恰好没有 `§3.3`，所以它是**悬空**的（新 R3 直接报红） | 同上 |
| [`loopx-scheduling-notes.md` §3.3](../../docs/design/loopx-scheduling-notes.md#33-难用的-9-条结构性根因有证据) 的标题写「8 条」，正文列了 **9** 条 | 第 9 条是后来补进去的（`（本项目实测）`那条），标题的计数没跟着改；`DESIGN.md` 引的正是「第 9 条」 | 标题改成 9 条 |
| 闸自己：节号下界写死 1 | 只管的那两份都从 `§1` 起，所以从没触发过。推广的一瞬间，所有从 `## 0.` 起编的文档（ADAPTIVE-REPLAN / LONG-RUN-PROOF / OPEN-SOURCE-REVIEW / SELF-IMPROVEMENT / loopx-\*）全体报一句「节号不连续，缺 §」——**缺的还是个空列表**，什么也没说 | 拆成两句话：起点只准 `§0` 或 `§1`；起点之后不准跳号 |

新增的两条约束：**起点只能是 `§0` 或 `§1`**（「从 `§2` 起」＝删了节没重编号，指向 `§1` 的引用从此无处可落），
以及**链接文字里的号要和锚点的落点一致**（写 `[正文 §20](../../docs/audit/CODE-AUDIT.md#x)` 就得真落在 `§20` 名下，
不能号写对、人却落到别节）。后者全仓跑下来 88 处、0 处不符——今天是绿的，但从此改错会红。

推广还逼出一个原来不存在的问题：**讨论规则的文字和遵守规则的文字长得一模一样。**
上面那张表里写「`DESIGN.md` 的「notes `§3.3`」」，闸照样把它当成一次真引用去查——
写这一节的时候当场红了 5 处，全是本节自己的举例。分得开的只有反引号：`§N` 待在**行内代码**里
＝在引用这个写法本身，不查（行内代码里的 `[链接](x.md#y)` 同理，R1 也豁免）。
这个豁免是量过才敢开的：全仓 187 处 `§` 里只有 2 处待在行内代码里，都是引用写法
（本节这条举例，和 `DESIGN.md` 里那段 JSON 载荷），一处真引用也没漏掉。

还有一件事这一轮才想明白：**闸绿着的时候，没有任何东西在回答「规则还灵不灵」**。
`the_doc_link_gate_is_green` 那个测试，在闸什么都不查时同样是绿的。所以加了
`python3 scripts/check-doc-links.py --self-test`：拿一组合成文档跑一遍，报出来的必须
**正好**是期待的那 9 条（t47 加了 R5，现在是 10 条）——少一条是规则失灵，多一条是规则误伤（假阳性同样会让人把闸关掉）。
`cargo test` 的 `the_doc_link_gate_rules_still_bite` 调的就是它。它上任第一件事就是抓住了
自己人写的两个 bug：上表最后一行那个下界，以及 macOS 的 `mkdtemp()` 给的 `/var/folders/…`
是符号链接、路径不 `resolve()` 就把合成文档判在「仓库外」，于是 R1/R3 全体静默跳过、
self-test 只剩空转（**一个永远绿的自检器**，比没有更糟）。

---

## 跨文档引用一律写成锚点链接（t47）

t46 那张表的第一行留了个尾巴。`DESIGN.md` 的「notes `§4.3`」之所以能指到别人家的同号节上，
根因不是「那一条写错了」，而是**这个写法本身就没法验**：闸只能靠「同一行、§ 之前最近提到的
`xxx.md`」去猜它说的是哪份文档。猜对了没人夸，猜错了也没有任何东西报错——t46 修的是那两条，
写法还留着，下一条同样写法的引用照样能烂。

所以 t47 把它立成一条**写作约定**，并且当场配上机械执行：

> 指别的文档的某一节，写 `[xxx.md §N](xxx.md#锚点)`；不要写 `xxx.md §N` 这种裸号。

两种写法被验的程度差一大截：

| 写法 | 归属：说的是哪份文档 | 落点：点进去到哪儿 |
|---|---|---|
| 裸号 `notes.md §3.3` | **猜**的（同一行最近提到的 `.md`），猜错不报错 | 没有落点 |
| 锚点链接 | 就是链接指向的那份，不用猜 | R1 验锚点存在，R3a 验「文字里的号」和「锚点落的那一节」是同一节 |

`scripts/check-doc-links.py` 因此多了第五条规则：

**R5 跨文档的 § 必须待在链接里。** `§N` 指的是本仓库**另一份**文档、又没待在链接里，就红。
豁免两种：目标文档不在这个仓库里（写不成链接，只能裸着引——loopx 上游的
`field-derived-patterns.md` 之类，共 4 处，跳过并计数），以及同一行提到的就是本文件
（那是自指，不是跨文档）。三级归属退回它本来的位置：**兜底**，只服务这两种豁免。

上任当场抓到 7 处，全仓一次改完：

| 文件 | 处数 | 原文 |
|---|---:|---|
| `README.md` | 2 | 一行里的 `§6–§10`（链接只包住文件名，号在链接外面） |
| `docs/CODE-AUDIT.md` | 1 | t46 那张表里讲 notes `§3.3` 的那一行——**讲这条毛病的句子自己就是这个毛病** |
| `docs/LONG-RUN-AUDIT.md` | 1 | 开头「见 `loopx-principles.md §1`」 |
| `docs/loopx-principles.md` | 3 | 两处指 notes 的 `§1`/`§3`，一处指 `§3.3` |

改完全仓 166 处 `§N` 的分布：**97 处待在链接里**（原 90）、69 处自指、4 处指向仓库外，
裸的跨文档引用 **0**。self-test 的期待值从 9 条加到 10 条，新增的两处覆盖是一对：
`docs/BROKEN.md` 里一条**号明明存在**的裸跨文档引用必须报（证明 R5 查的是写法不是号），
`docs/DESIGN.md` 里一条「同一行提到的就是自己」必须不报（证明它不会把自指误伤成跨文档）。
另外把合成文档里原来那条「好的跨文档引用」改成了链接形式——它在旧规则下是正面样例，
在 R5 下就是违规，不改的话 self-test 会自己报一条多余的。
