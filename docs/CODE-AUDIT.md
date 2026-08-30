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
| CLI 参数 · 巨大数字 | `compact --keep-days` / `doc --since\|--until` | 💥 **A-8**：装得下 i64 就 panic |
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
| state.json · 数值越界 | `policy.window_hours` 手改大 | 💥 **A-7**：`next`/`status`/`context` 全 panic |
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

`RunArgs` 说得很清楚：`Exit when waiting on a human instead of polling at the slowest interval`。
实测反过来。

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
    let m = d.interval_min.unwrap_or_else(|| slowest_interval(state));
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
见 [A-16](#a-16中高noop-计数从交互式命令串进-runner-的停机判断人敲三下-zloop-next-就能让长跑拒绝启动--已修)。

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

### A-7（中）`policy.window_hours` 手滑一下，`next` / `status` / `context` 全 panic，而 `doctor` 说没问题

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

### A-8（中）时间参数「装得下 i64」就 panic，装不下反而有好错误提示

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

### B-2（低）`edit <id> --blocked-by <它自己>` 被收下，那条 todo 就再也跑不了

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

## 6. 第四轮：调度逻辑的边界

全部用真命令构造，`zloop next --json` 读结果。

| 场景 | 实际行为 | 判断 |
|---|---|---|
| 0 条 todo（刚 init） | `unplanned` + "还没有待办 · 先用 zloop plan" | ✅ |
| 全部 deferred | `done` + "0 条待办全部完成，目标结束（另有 2 条延后）" | ❌ B-3（已修） |
| 自依赖 `t1←t1` | `blocked`，"30 分钟后重试"，doctor 沉默 | ❌ A-9 |
| 二元环 `t1←t2←t1` | 同上 | ❌ A-9 |
| tick 时间戳在 2099 | `ready`（不撞配额时无影响） | ✅ |
| tick 时间戳在 1970 | `ready` | ✅ |
| tick 时间戳是乱码 | `ready`，不崩 | ✅ |
| **tick 在未来 + 撞配额** | `throttled`，`interval_min=38048610`（72 年） | ❌ **A-11**（已修） |
| `max_runs = -5` | 反序列化就拒绝："expected usize"，exit 1 | ✅ |
| 五个阈值分别设 0 | 三个当"关闭"，两个当"永远触发" | ❌ A-10 |

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

### A-9（中高）依赖成环没人拦，永久卡死且无诊断

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

### A-10（中）"0 = 关掉这个检查"只对三个阈值成立

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

## 7. 第五轮：实景撞上的

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

## 8. 第六轮：A-6 的同一类死法，另外三条路

A-6 修的是宿主/preflight 那条路（`run_with_timeout`：进程组 + 排水上限）。
这一轮沿着同一个问题问下去：**runner 每轮还起了哪些子进程，它们有闸吗？**

### 8.1 全部子进程调用点

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

## 9. 第七轮：noop 计数的作用域

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
    let m = d.interval_min.unwrap_or_else(|| slowest_interval(state));
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

## 10. 第八轮：把「交互式命令写进账本的东西」系统扫一遍

A-16 只是一个样本。这一轮把问题反过来问：**runner 的判断一共读账本里的哪些量，
这些量分别有谁在写？** 只要写的人里有一个是交互式命令，那就是一条 A-16 同型的路。

### 10.1 runner 读什么（判断输入的全集）

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

### 10.2 谁在写（写命令的全集）

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
| `compact` | **删 todo + 删 tick** | 4 5 **7** 8 | → A-18 |
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

### A-18（中）`zloop compact` 把花费一起归档走，`max_total_usd` 静默复位

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

修的方向：把已归档的花费/轮次留一个汇总在 `state.json` 里（例如
`policy.spent_before_compact`），`spent_usd` 加上它；或者 compact 时显式提示
「这次整理让已记录花费从 $X 降到 $Y，预算闸会跟着放开」，让人自己决定。

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

### 10.3 检查过、确认不是问题的

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

## 11. 第九轮：A-17 那张表上同型的最后两条

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

### 11.1 这一类到此为止

「交互式命令写进账本的东西串进 runner 的判断」这一类（A-16 → A-17 → A-18 / A-19 → A-20 / A-21）
到这一轮为止，`tick.rs` 里三条 streak 的规矩统一成了一句话：

> **人写的 tick（`feedback` / `edit`）只有在循环已经停在这条 streak 上时才清零；
> 还在跑的时候，只有「`edit` 改的正是当事的那条 todo」算数。**

`noop_streak` 不在此列：它不是停机闸，A-16 之后 runner 也不读它。

A-19（`pick_session` 认错会话）跟 streak 无关，但归到同一句话下面：它串的不是停机判断，
是「这一轮 `--resume` 谁」，判据换成同一个 `tick::is_writeback` 就修好了——
**人写的 tick 不是宿主跑过的一轮**，三条 streak 和会话谱系都按这一句办。
这一类剩下没修的只有 A-18（`compact` 抹掉花费），在第 10 节。
