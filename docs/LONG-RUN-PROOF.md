# 怎么证明 zloop 真的在跑长程任务

> 起因：用户看完连着几轮"长程任务"之后问——**"现在长程任务好像全是短程任务呀，有什么方法可以验证确实在执行长程任务吗？"**
> 查完账本，这个怀疑是对的。本文先摆事实（§0），再给判据（§1）、自检器（§2）、
> 工作流（§3），最后是**真跑那一次的记录**（§4，2026-08-29，17 轮 / 4 小时 / 0 干预）。

## 0. 先摆事实：提出这个问题时，zloop 从没在这个项目里真正长跑过

> 本节写于 2026-08-28，保留原样——它是这份文档的起点。§4 是它被推翻的那一天。

| 证据 | 数字 |
|---|---|
| 全部 tick（含停放和归档的目标） | **62 条，`host` 全是 `claude`，2 个会话** —— 全部是交互轮次 |
| runner 的 journal | **6 行**，来自 2026-08-28 07:58 的两次调用 |
| 那两次调用干了什么 | `awake_on` → `awake_off` → `stop (done)`，相隔 4 秒，**一轮都没跑** |
| GitHub issue | 0 个 |

所以"长程"此前的真实状态是：**有测试覆盖，零真实证据**。
`tests/runner_test.rs` 里的 17 条（挂死超时、限流退避、会话谱系、等人轮询、预算透传、
无头回看、无头重估……）全部用**假宿主**——它们证明的是"逻辑对"，不是"真跑过"。

**"测过"不等于"证明过"。** 这份文档存在的意义就是把这条线划清楚。

## 1. 判据：只认 zloop 自己伪造不出来的东西

zloop 的账本是它自己写的，"跑了 9 轮"由它自己说不算数。所以判据只取那些
**必须真的发生过一次无头长跑才会留下的痕迹**，并尽量落在 zloop 之外：

| # | 判据 | 为什么它不好伪造 |
|---|---|---|
| 1 | runner journal 里 ≥N 组 `begin`/`end` | 这两个事件**只有 runner 每轮才写**；交互轮次一行都不写 |
| 2 | 墙钟跨度 ≥H 小时 | 第一个 `begin` 到最后一个 `end`，时间戳是逐轮追加的 |
| 3 | 窗口内 0 条人工 tick（`edit` / `feedback`） | 有人插手就不叫"无人值守" |
| 4 | 第 2 轮起接续上一轮会话（`begin.resume`） | 证明跨轮连续，而不是七次独立的短跑 |
| 5 | ≥1 轮带宿主回报的 `cost_usd` / `duration_ms` | 这两个数由 `claude -p` 返回，没真调过就没有 |
| 6 | 窗口内有 git 提交 | **完全在 zloop 之外**，它伪造不了 |

默认阈值 N=6 轮、H=2 小时，可用 `--rounds` / `--hours` 调。

## 2. 自检器

```bash
scripts/longrun-audit.py [--dir PATH] [--rounds N] [--hours H] [--all]
```

只读：不写任何文件、不改任何状态。逐条打勾，全过才判"是长程"，退出码 0 / 1 可进 CI。
它同时读当前目标、停放的目标和归档的目标——长跑可能发生在任何一个目标上。

**窗口取最近一次运行，不是整个 journal。** journal 是追加的，同一个项目里 runner
起停过很多次；拿整份来量，会把上一次运行之前的人工 tick 和提交算进这一次的窗口。
`--all` 才是累计口径。这条是踩出来的，见 §4.5。

当时拿本仓库现状跑（就是 §0 那张事实表的机读版）——`--all`，因为那会儿一次真运行都没有：

```
  窗口：2026-08-28 07:58 → 2026-08-28 07:58
  ❌ runner 驱动的轮次    0 轮（要求 ≥ 6）
  ❌ 墙钟跨度             0.00 小时（要求 ≥ 2.0）
  ✅ 窗口内无人工干预     0 条人工 tick（edit/feedback）
  ❌ 跨轮会话连续         0/0 轮接续了上一轮的会话
  ❌ 宿主回报了花费/耗时  0 轮带 cost/duration
  ❌ 窗口内产出 git 提交  0 个提交

  结论：**不是**长程运行
```

**一个永远说 no 的检查器毫无价值**，所以也验了它会说 yes：合成一份 7 轮、跨 3.42 小时、
带 resume 链和 cost、窗口内有一个提交的现场，六条判据全绿、退出码 0。
（修完窗口 bug 之后这份合成现场扩成了三段——空跑 / 昨天带人工 tick 的一次 / 今天这次——
用来验它挑对了段，见 §4.5。）

同一条命令今天跑出来的样子在 §4.2。

## 3. 工作流：GitHub issue ↔ zloop todo

`scripts/gh-issues.py`，三个子命令：

```bash
scripts/gh-issues.py pull  [--label L] [--priority P] [--apply]   # open issues → 带 (#N) 的 todo
scripts/gh-issues.py close [--yes]                                 # 已完成的 → 评论 + 关闭
scripts/gh-issues.py status                                        # 对一遍：哪条 todo 绑了哪个 issue
```

**为什么是脚本，不是 zloop 子命令**：zloop 的承诺是"一个 JSON 文件、十几条命令、不依赖外部服务"。
把 GitHub 拉进去意味着它要管认证、网络错误、API 变更——那不是调度器该操心的事。
而 todo 的文本本来就是自由的，把 `(#12)` 写进去就够当链接用了，**零 schema 改动**。

三个安全设计：

| 设计 | 为什么 |
|---|---|
| `close` **只对 `status == "done"` 的 todo 动手** | progress / fail / blocked / deferred 一律跳过——没做完就关 issue 是最糟的失败模式 |
| `close` 默认只**预览**，`--yes` 才真动手 | 和 `reflect --apply` 同一个原则：会对外产生副作用的动作要显式 |
| `pull` 跳过已绑过的 issue，`close` 跳过已 CLOSED 的 | 长跑里这两条会被反复调用，必须幂等 |

评论内容取自账本：一句话结果、过程记录的日志路径、完成时间、验收标准。

### 端到端实测（真仓库，[#1](https://github.com/zouhuigang/zloop/issues/1)）

```
$ scripts/gh-issues.py pull --apply
[P1] 冒烟：验证 issue → todo → done → 自动评论并关闭 这条链路 (#1) :: 这条 issue 被脚本自动评论并关闭…
t5 [P1] …                                    ← 连 issue body 里的「验收：」都抽成了 acceptance

$ scripts/gh-issues.py close --yes            ← t5 还没做完
  跳过 #1（t5 是 open，没完成）
$ gh issue view 1 --json state --jq .state
OPEN                                          ← **失败路径不误关**

$ zloop done t5 --note "…" --approach "…"
$ scripts/gh-issues.py close --yes
  ✅ 关闭 #1（t5）

$ gh issue view 1 --json state,closedAt
{"state":"CLOSED","closedAt":"2026-08-29T00:52:19Z"}   ← GitHub 侧核实

$ scripts/gh-issues.py close --yes  &&  scripts/gh-issues.py pull
  跳过 #1（issue 已经是 CLOSED）
没有符合条件的 open issue                     ← 两条都幂等
```

## 4. 实测：2026-08-29，09:04 → 13:05

判据、尺子、工作流都齐了之后，真跑了一次。**结论：过了**——用自检器的默认门槛
（≥2 小时、≥6 轮）六条判据全过，`exit 0`。用当天临时设的 4 小时门槛，
跨度差 **39 秒**没够（下面照实记）。

### 4.1 怎么摆的局

- 从已知 backlog 在真仓库建了 **13 个 issue（[#2](https://github.com/zouhuigang/zloop/issues/2)–[#14](https://github.com/zouhuigang/zloop/issues/14)）**，
  `scripts/gh-issues.py pull` 拉成带 issue 号的 todo
- `zloop start` 无头驱动，`.zloop/runner/autostop.sh` 定时 4 小时后 `zloop stop`
- 开跑前把三条约定钉进 `.zloop/NOTES.md`（每轮都注入交接包）：

  ```
  - 本次长跑：只 git commit 到本地，**绝对不要 git push**，也不要关闭 GitHub issue——修复还没被人看过。
  - 改动必须 cargo test 全过才写回；测试挂了就 --outcome fail 并把报错放进 --pitfall。
  - 一轮只做一条 todo。做完就写回，不要顺手改别的 issue 的代码。
  ```

- 全程不介入。Stop hook 在这 4 小时里反复把 runner 正在做的那条 todo 推给交互会话，
  一次都没接——两个 agent 同时改一批文件必然互相踩
  （这本身就是 [#14](https://github.com/zouhuigang/zloop/issues/14)：`cmd_hook_stop` 绕过了 `held_by_other`）。

### 4.2 自检器怎么判的

```
$ python3 scripts/longrun-audit.py
  长程自检 · /Users/zouhuigang/work/cc/zloop
  窗口：2026-08-29 09:04 → 2026-08-29 13:03

  ✅ runner 驱动的轮次    17 轮（要求 ≥ 6）
  ✅ 墙钟跨度             3.99 小时（要求 ≥ 2.0）
  ✅ 窗口内无人工干预     0 条人工 tick（edit/feedback）
  ✅ 跨轮会话连续         1/16 轮接续了上一轮的会话
  ✅ 宿主回报了花费/耗时  21 轮带 cost/duration
  ✅ 窗口内产出 git 提交  16 个提交

  结论：这是一次长程运行
```

窗口 `09:04:01 → 13:03:22`，即 3 小时 59 分 21 秒，宿主回报花费合计 **$57.62**。
把门槛提到 `--hours 4` 就会挂在跨度那条——**差 39 秒**，不进位。
差这一点不是巧合：自动停机设的正是起跑 +4 小时，最后一轮 13:03:22 收工、13:05 收到 SIGTERM。

### 4.3 时间线

`round` 是 runner 自己的轮次计数（第 1–4 轮是这次长跑之前的交互轮次）。
第 12 行那条 `resume ✔` 是同一条 todo 跨轮续跑——上一轮写的是 `progress`，
runner 用 `claude --resume` 把上下文接了回去。

| # | round | todo | 起 | 耗时 | issue |
|---:|---:|---|---|---:|---|
| 1 | 5 | t6 | 09:04 | 7.9m | [#13](https://github.com/zouhuigang/zloop/issues/13) |
| 2 | 6 | t18 | 09:14 | 6.1m | — |
| 3 | 7 | t7 | 09:24 | 12.6m | [#12](https://github.com/zouhuigang/zloop/issues/12) |
| 4 | 8 | t19 | 09:39 | 16.4m | — |
| 5 | 9 | t8 | 09:58 | 10.6m | [#11](https://github.com/zouhuigang/zloop/issues/11) |
| 6 | 10 | t9 | 10:12 | 14.2m | [#10](https://github.com/zouhuigang/zloop/issues/10) |
| 7 | 11 | t10 | 10:29 | 8.4m | [#9](https://github.com/zouhuigang/zloop/issues/9) |
| 8 | 12 | t20 | 10:41 | 16.4m | — |
| 9 | 13 | t11 | 11:00 | 5.9m | [#8](https://github.com/zouhuigang/zloop/issues/8) |
| 10 | 14 | t12 | 11:09 | 6.4m | [#7](https://github.com/zouhuigang/zloop/issues/7) |
| 11 | 15 | t21 | 11:18 | 8.4m | — |
| 12 | 16 | t21 | 11:30 | 2.6m | — `resume ✔` |
| 13 | 16 | t13 | 11:39 | 8.5m | [#6](https://github.com/zouhuigang/zloop/issues/6) |
| 14 | 17 | t14 | 11:51 | 7.7m | [#5](https://github.com/zouhuigang/zloop/issues/5) |
| 15 | 18 | t15 | 12:06 | 14.7m | [#4](https://github.com/zouhuigang/zloop/issues/4) |
| 16 | 19 | t16 | 12:29 | 15.3m | [#3](https://github.com/zouhuigang/zloop/issues/3) |
| 17 | 20 | t17 | 12:51 | 11.8m | [#2](https://github.com/zouhuigang/zloop/issues/2) |

结果分布：`done` 15 · `progress` 1 · `block` 1 · `replan` 4 · `noop` 1。

journal 摘录（一轮的完整形状）：

```json
{"event":"begin","round":16,"todo":"t21","host":"claude","resume":"<session-id>","at":"2026-08-29T11:30:…"}
{"event":"end","round":16,"at":"2026-08-29T11:32:…"}
{"event":"replan","after_round":16,"at":"2026-08-29T11:36:…"}
{"event":"sleep","until":"…","reason":"interval","at":"…"}
```

### 4.4 它自己长出来的活

清单里只有 issue 拉来的 12 条。**t18 / t19 / t20 / t21 是 runner 边跑边排的**——
`zloop done --next` 记下来，下一轮自己捡起来做：

- **t18** — `cargo test` 在 zloop 自己的 runner 里跑会挂两条 `hook_stop` 测试：
  runner 给子进程设了 `ZLOOP_RUNNER=1`，测试继承了它。修法是测试助手统一清掉继承来的会话环境变量。
- **t19** — 回看那轮的两处账目：runner 只把宿主输出前 300 字写进 `.zloop/log/*-reflect.md`；
  `zloop status` 的「跑了 N 轮」把 `reflect` 这类不计数 tick 也算了。
- **t20** — `start` 之后立刻 `status` 偶发看不到 runner。**根因是 pid 文件用 `fs::write` 覆写
  （先截断再写），`status` 正好读在中间读到空文件**；改成同目录 tmp + rename 原子替换。
  修复前 20000 次探测误报 5469 次，修复后 0 次。
- **t21** — 「本机的 /zloop 还是旧模板」。这条它写的时候就在正文里注明「现在别做，
  会在长跑中途换掉 runner 正在用的 skill」。**轮到它时它没有照做**：改在 `/tmp` 的沙箱 HOME
  里把整个升级流程彩排了一遍（拷 SKILL.md、比对托管区/用户区/指纹、查出本机二进制落后 13 个提交），
  再用 `--block` 退出轮转来问人。

t6（[#13](https://github.com/zouhuigang/zloop/issues/13)）也值得记一笔：todo 只要求"补极端情况的测试"，
它先去探真实输出，发现 **3 条验收标准里有 2 条根本不成立**，于是连实现一起修了
（保护区被穿、半个章节、`budget=0` 超预算 1 字符）。

### 4.5 最能说明问题的一件事：它发现自己的尺子是坏的

第 17 轮后的自动重估里，它重算了窗口，写下：

> | 真正的无头窗口 | `begin` 首条 09:04:01 → `end` 末条 11:58:56，**2.92 小时，14 轮** |
> | 窗口内人工 tick | **0 条**（3 条 edit 在 09:01–09:03，长跑开始之前） |
> | 自检器当前判定 | ❌ 不是长程 —— 唯一挂掉的那条是「窗口内无人工干预：6 条」 |

也就是说：**无人看管的时候，它自己查出 `scripts/longrun-audit.py` 把窗口取成了整个 journal**，
于是把长跑*开始之前*的人工 tick 算进了「窗口内」，给出假阴性。
它在更早一轮就说过「最要紧的一条是——目标里那把『zloop 之外的尺子』现在给出的是假阴性，
而没有任何一条 todo 管这件事」。

这个 bug 已修（`5d3cf47`）：`runs()` 按 `awake_on` / `restart` 把 journal 切成一次次运行，
只量最近那次；跨度只认 `begin`/`end` 的时刻（段尾的 `sleep` / `stop` 在最后一轮之后，
算进去等于给自己送时间）；`--all` 保留累计口径。
修完验过它**还说得出 yes**——造一份三段 journal（空跑 / 昨天 3 轮 5.5h 带人工 tick / 今天 7 轮），
默认只量最后一段并给出 `exit 0`。**一把只会说 no 的尺子和一把只会说 yes 的尺子一样没用。**

### 4.6 这次跑出来的缺陷

| # | 缺陷 | 状态 |
|---|---|---|
| 1 | 自检器窗口取整个 journal，给假阴性 | ✅ 已修 `5d3cf47` |
| 2 | `blocked` 是持续状态不是事件：t21 挂着等人回话期间，之后每轮都触发一次重估，4 次全是同一个信号 | 待修 |
| 3 | `replan.rs:105` 的 blocked 信号不排除 `status == done`：todo 做完了 `blocked_by` 还留着 `user`，这个信号会永远响下去 | 待修 |
| 4 | 重估/回看记录被截到 300 字，4 份重估的结论全丢了后半段（t19 已在源码修掉，跑的是旧二进制） | ✅ 源码已修，需重装 |
| 5 | Stop hook 绕过 `held_by_other`，runner 在跑时仍催交互会话抢同一条 todo | [#14](https://github.com/zouhuigang/zloop/issues/14) |

### 4.7 没做到的

- **issue 没有被自动关闭**，13 个 issue 至今全 OPEN。这是开跑前钉的约定主动禁掉的
  （"修复还没被人看过"），不是漏做——代价是 t3 的第四条验收当场就注定过不了。
  链路本身在 [#1](https://github.com/zouhuigang/zloop/issues/1) 上冒烟验过能走通。
- **18 个 commit 还在本地**，同上。

所以这次证明的是：**zloop 能无人看管地跑 4 小时、17 轮、产出 16 个真提交，
并且在此期间发现并修掉自己的 bug——包括量它自己的那把尺子。**
"推送 + 关 issue"这半截是人为掐掉的，不是它做不到。
