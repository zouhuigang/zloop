# 怎么证明 zloop 真的在跑长程任务

> 起因：用户看完连着几轮"长程任务"之后问——**"现在长程任务好像全是短程任务呀，有什么方法可以验证确实在执行长程任务吗？"**
> 查完账本，这个怀疑是对的。本文先摆事实，再给判据和自检器。

## 0. 先摆事实：截至本文，zloop 从没在这个项目里真正长跑过

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
scripts/longrun-audit.py [--dir PATH] [--rounds N] [--hours H]
```

只读：不写任何文件、不改任何状态。逐条打勾，全过才判"是长程"，退出码 0 / 1 可进 CI。
它同时读当前目标、停放的目标和归档的目标——长跑可能发生在任何一个目标上。

拿本仓库现状跑（就是上面那张事实表的机读版）：

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

## 4. 接下来（t3）

判据、尺子、工作流都有了，还差**被测量的那件事**：从已知 backlog 建几个真 issue，
`zloop start` 无头跑，全程不干预，然后拿 `scripts/longrun-audit.py` 量。

在那之前，任何"zloop 能跑长程"的说法都只是设计意图。
