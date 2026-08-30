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
| NOTES.md · 非 UTF-8 | 追加一个 `\xff`；粘一行 GBK | 💥 **A-4**：静默清零 + 下一次写入真删 |
| 子进程 · 超时 | preflight / 宿主留下后台进程 | 💥 **A-6**：超时形同虚设，SIGTERM 也叫不动 |
| 信号 · SIGPIPE | `zloop status \| head` | ✅ `main.rs` 恢复了默认处置，安静退出 |
| 信号 · SIGTERM | `zloop stop` 打断正常轮次 | ✅ 干净退出并记 journal（已有测试） |

### A-4（高）NOTES.md 里一个非 UTF-8 字节 → 约定和经验静默清零，下一次 `remember --rule` 把它们真删掉

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

修法三条，缺一不可：
1. `notes::read` 区分「文件不存在」（返回 default 是对的）和「读失败」（要说话）；
2. 读失败时 `add_rule` / `replace` **拒绝写**，不许在空 `Notes` 上重建文件；
3. `doctor` 加一条：NOTES.md 存在但读不出来 → 报出来并说该怎么办。

### A-5（高）`--exit-on-wait` 在「等人」时从不生效——它只在一种 runner 自己走不到的状态下才管用

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

修法：`wait_plan` 把 `exit_on_wait` 提到 `interval_min` 前面判——
「等人」这件事该不该退出，由标志决定，不该由 noop 计数决定。
顺带把那个测试的三次 `zloop next` 去掉，让它钉真实路径。

### A-6（高）超时管不住留下后台进程的那一轮，而且这段时间里 SIGTERM 叫不动 runner

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

修法两条：
1. 给子进程单开一个 process group（`pre_exec` 里 `setpgid`），超时时 `killpg` 整组，
   孙进程一起收掉，EOF 自然来；
2. 排水也要有上限：`join()` 之前给读线程一个 deadline，超了就放弃这一轮的输出
   （宁可少记一段 stdout，也不能让 runner 停在这里）。

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
