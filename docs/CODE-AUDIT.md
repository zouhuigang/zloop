# 全量代码审查（2026-08-29）

> 目标原话是"确保代码没有 bug 和漏洞"。**这做不到**——不存在证明不了。
> 这份文档能给的是：按风险面逐块过一遍，每条发现都带**可复现的失败场景**；
> 试过但复现不出来的，写明"试过没复现"，不写进 issue，免得污染真发现。

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

见 §2 的 A-1。这是这一轮唯一确认的 panic。

**（二）靠不变量保证，成立 —— `phase.rs:154`、`cli.rs:742`、`cli.rs:767`、`runner.rs:634`**

四处都是 `decide()` 返回 `should_run` 之后直接 `todo.unwrap()`。核过：
`tick.rs` 里 `should_run: true` **只有一处**构造，且同时 `todo: Some(...)`，所以成立。

但 `Decision` 是 `pub` 结构体、字段全 `pub`，这个不变量**没有任何东西守着**——
将来谁手工构造一个 `Decision { should_run: true, todo: None, .. }`，四处一起崩。
低危，记在 §2 的 B-1。

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

## 3. 测试覆盖空白

用"pub 函数名在 `tests/` 里一次都没出现"粗测（会低估：很多函数是通过 CLI 测试间接跑到的）：

| 模块 | pub fn | 测试没提到 | 其中值得注意的 |
|---|---:|---:|---|
| goals | 14 | 7 | `sanitize_id` `fresh_id` `resolve_match` |
| tick | 17 | 6 | `pending_feedback` `failures` `noop_streak` |
| log | 11 | 6 | `resolve_evidence` `read_section` |
| awake | 12 | 8 | 多数是平台相关，测不了 |
| session | 5 | 4 | `detect` `transcript_path` |
| **hosts** | 5 | **2** | **`install_claude_stop_hook`** ← 就是 A-1 那个 |
| phase | 3 | 3 | `compute` `reason_zh` |

**最值得记一笔的是最后一列那个巧合**：这一轮唯一确认的 panic 就在
`install_claude_stop_hook` 里，而它是 `hosts.rs` 五个 pub 函数中**没被任何测试提到**的两个之一。

## 4. 发现清单

严重度按「会不会让用户的东西坏掉 / 会不会让人看到错的结论」排，不按修起来难不难。

### A-1（高）`zloop install --claude-stop-hook` 在 settings.json 结构不对时 panic

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

### B-1（低）`Decision` 的 "should_run ⇒ todo 非空" 不变量没人守

四处 `unwrap()` 依赖它，今天成立，但结构体字段全 `pub`，没有构造器也没有 `debug_assert`。
不是 bug，是个绊子。修法：加一个 `Decision::ready(todo)` 构造器，或在 `decide()` 出口加
`debug_assert!(!d.should_run || d.todo.is_some())`。

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

### A-2（中）`zloop remember --rule` 并发会丢条目

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

### A-3（低）被杀之后留下 `state.json.tmp`，`doctor` 说"没发现问题"

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
