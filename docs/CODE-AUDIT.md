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
