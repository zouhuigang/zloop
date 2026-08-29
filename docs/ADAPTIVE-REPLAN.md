# 自适应重规划：做完一条就重估后续

> 目标：`每条 todo 完成之后都重估一次剩余任务，该改就改，以便真正达成最终目标`。
> 本文是 t1 的调研产出：抄什么、不抄什么，以及收敛成 zloop 能落地的三条做法。

## 0. 先说一个意外：这件事 **Warp 没有**

用户的原话是"参考 warp 的自适应"，但查下来 Warp 的 Plan Mode（[Agents 3.0][warp-a3]）明确是
**执行前的对齐检查点**，不是执行中的自适应修订：

> "You and the agent agree on not just what to build but also how to build it **before execution begins**."
> "Each change creates a new version so you can track changes to the plan over time."

它做的是：`/plan` 生成计划 → 人 review / 编辑 / 让 agent refine → 定稿后再开工；计划**持久化**成项目资产
（跨会话、可以走 PR 或 Warp Drive 分享）。整篇没有"执行中按信号自动修订计划"这回事。

Warp 值得抄的是**另外两点**，而且 zloop 已经有了：
- 计划是**人和 agent 共同定稿**的（zloop：`plan` + 人点头）；
- 计划**跨会话持久化**、每次改动留痕（zloop：`state.json` + tick 账本）。

所以"做完一条就重估后续"这件事得从别处找依据。

## 1. 文献给的四条机制

| 机制 | 出处 | 说的是什么 |
|---|---|---|
| **验证驱动的重规划** | [Plan-Execute-Verify-Replan][pevr] | 编排层有一个 verifier 评估"结果完整吗"，**发现缺口才触发**重规划——信号和具体 agent 实现解耦 |
| **选择性触发** | [Bayesian Partner Modelling][bayes] | 用"当前信念下的矛盾"来选择性打断；**"attains comparable reward with far fewer replans than heuristic or LLM-based triggers"** |
| **修复 vs 重来** | [Plan commitment: replanning versus plan repair][repair] | 不同失败信号该有不同动作：超时→退避重试、上下文过期→刷新、参数错→就地修、证据矛盾→交叉验证。plan repair **承诺原计划**，改动更小 |
| **进展门控** | [ReflexGrad][reflex] | 平时走快路径，**停滞时**才切到慢路径做因果重规划：输出一段短计划，点名可疑根因和纠正动作 |

## 2. 最反直觉、也最重要的一条

**别每轮都重规划。** [Bayesian][bayes] 那篇是直说的：选择性触发能用**远少于**启发式或 LLM 触发的重规划次数，
拿到相当的收益。每轮都调一次模型重估剩余任务，代价是钱和时间，收益却不是线性的——
更糟的是它会**制造**计划抖动：模型每被问一次"要不要改"，就有概率改一点。

所以用户那句"每个 todo 完成之后都看看需不需要修正"，落地形态应该是：

> **每轮都做一次「便宜的体检」（纯代码、不调模型）；只有体检命中信号，才升级成一次真正的重规划提案。**

这和 zloop 已有的做法是一路的：`reflect` 的机械体检也是这个形状（代码能判断的用代码，判断交给模型，落地要人点头）。

## 3. zloop 手上已经有的信号（一个都不用新造）

| 信号 | 已有的出处 | 说明什么 |
|---|---|---|
| 用户反馈 | `tick.outcome == feedback`（W1） | **最强信号**：人明确说了"这样不对" |
| 停滞 | `progress_streak`（同一条 todo 连续 progress） | ReflexGrad 说的 stalled，快路径走不动了 |
| 连续失败 | `fail_streak` + 那几轮的 `pitfalls`（W4） | 方法可能整个错了，不只是这一条难 |
| 返工率 | `stats.rework_rate` / `roughest`（W5） | 计划的颗粒度可能不对（一条塞了太多事） |
| 被挡 | `outcome == block` | 计划里有需要人决定却没拆出来的分叉 |
| 新发现 | `done --next` 插进来的后继 | 干着干着发现原计划漏了东西 |
| 验收未过 | `todo.acceptance` 写了但没对照 | 验证驱动那一路：结果不完整 |

**结论：体检完全可以是纯代码的**，因为这些信号全都在账本里。

## 4. 收敛：zloop 要落地的三条

1. **`done` 之后跑一次便宜体检**（不调模型）。命中信号就在 `done` 的输出里多打一行，
   例如：`⚠ t3 连续 3 轮 progress、t5 有你的反馈没处理 —— 后续任务可能要调整：zloop replan`。
   没命中就什么都不说——**沉默是默认**，这是"far fewer replans"的直接实现。
2. **`zloop replan`**：把目标、剩余 todo、这一轮学到的（note/pitfall）、触发的信号摆成一页给模型，
   要它提出**最小改动**（改哪条、加哪条、删哪条、拆哪条），而不是重开一张清单——
   这就是 plan repair 优先于 full replan。落地仍然走现成的 `zloop plan --add` / `zloop edit`，**人点头才改**。
3. **无头 runner 不自作主张**：`--reflect-every` 那条路已经定了调子——没人点头就只把建议记进账本。
   重规划同理：runner 可以生成建议，但**绝不自动改 todo**。计划是人和 agent 共同定稿的东西（这点抄 Warp）。

## 5. 明确不抄

- **每轮都调模型重估**：贵，且制造计划抖动（§2）。
- **全量重规划**：优先 plan repair，改动越小越可控（§1）。
- **自动改 todo**：zloop 的所有落地动作都要人点头，重规划没有理由破例。
- **验证器 agent**：[PEVR][pevr] 那套是多 agent 编排层的东西；zloop 单 agent，
  "验收标准"（`todo.acceptance`）已经是同一个位置的轻量版。

[warp-a3]: https://www.warp.dev/blog/agents-3-full-terminal-use-plan-code-review-integration
[pevr]: https://arxiv.org/html/2603.11445v2
[bayes]: https://arxiv.org/html/2608.18490
[reflex]: https://arxiv.org/pdf/2511.14584
[repair]: https://www.researchgate.net/publication/370067696_Plan_commitment_Replanning_versus_plan_repair

---

# 落地（t2）：体检 + `zloop replan`

## 两层，和 §2 的结论一一对应

| | 谁在跑 | 调模型吗 | 什么时候出声 |
|---|---|---|---|
| **体检** `replan::signals()` | 每次 `done` 之后自动 | 不 | **只有命中信号才打一行**，否则一声不吭 |
| **重估** `zloop replan` | 人（或模型）主动敲 | 是（材料给模型） | 命中之后才值得升级到这一步 |

`done` 的提示长这样，没命中就完全不出现：

```
⚠ 计划可能要调整：t2 有你的反馈（已经过了一轮） · t2 连续 4 轮没做完 · 返工率 80%（最费劲的是 t2）
  想清楚剩下的任务还对不对：zloop replan
```

## 五个信号，全部来自现成字段

| kind | 阈值 | 取自 |
|---|---|---|
| `feedback` | 还没做完的 todo 上出现过反馈 | W1 的 `outcome == feedback` |
| `stalled` | 同一条连着 ≥2 轮 progress | 账本 |
| `fail_streak` | 连续 ≥2 轮 fail | `tick::fail_streak` |
| `rework` | 返工率 ≥50% 且已跑 ≥3 轮 | W5 的 `stats` |
| `blocked` | 有 todo 在等人回话 | `blocked_by` |

## 两个刻意的决定

**1. 「在拖」和「要不要停下来」是两个问题，不共用一个数。**

`tick::progress_streak` 是"停下来等人"的闸，人一开口就该清零——给它带着新信息再试一次（W1 就是这么定的）。
但重估问的是"这条是不是在拖"，**人说了句话并不会让它不拖**。所以 `replan::dragging()` 单独数一遍，
把非干活的轮次（noop / reflect / feedback / edit）一律当透明。

同理，`feedback` 信号的范围是"**还没做完**的 todo 上出现过反馈"，而不是 `pending_feedback`（只算最近一轮之后的）：
人说完方向不对、agent 又干了一轮，`pending` 就空了——可那恰恰是最该重估的时刻。**实测时就是这么发现的**：
给完反馈再做一轮，两个信号一起消失了。

**2. 材料包里要给**人的原话**，不能只说"有反馈"。** 模型要判断的正是"人说的"和"计划里写的"差在哪。

## 人点头才算数

`zloop replan` **只读**，不改任何状态。它要模型做的是：

1. 对着**最终目标**看剩下的任务还能不能做成、漏了什么、哪条没意义了；
2. 只提**最小改动**（改文本 / 加一条 / 延后 / 拆开），**别重开一张清单**——plan repair 优于 full replan；
3. 逐条讲清为什么，讲给用户听；
4. 人点头之后用现成命令落地（`plan --add` / `edit --text|--acceptance|--priority|--status deferred`）；
5. **"不用改"是完全合格的结论**——这一条写在提示词里，专门防"为了改而改"。

## 验证

`cargo test` 91 passed / 0 failed，新增 `done_only_nudges_a_replan_when_the_ledger_says_something_is_off`：
顺利的一轮一个字不多说 / 连续两轮没做完触发停滞 / 人开口后信号不消失 / 材料包含目标+剩余（带验收）+三类信号+原话 /
提示词里有"别重开一张清单""人点头之后""不改是完全合格的结论" / `replan` 跑两次输出一致且 `state.json` 逐字节不变 /
全做完之后不再提示。

---

# 无头侧（t3）：runner 可以提议，但绝不改计划

## 触发方式和 reflect 不一样

| | 触发 | 理由 |
|---|---|---|
| `--reflect-every N` | **固定节奏**（每 N 轮） | 整理经验是攒够量才有得整理的事 |
| 重估（默认开，`--no-replan` 关） | **信号触发** | 计划没偏就别动它——每轮都重规划会制造抖动（§2） |

runner 在**写回成功之后**看一眼 `replan::signals()`：空的就什么都不做；非空就插一轮重估，
一轮活最多跟一次（用 `replan_at == round_no` 卡住，否则信号还在、下一轮又会触发）。
重估轮次记成第 9 个 outcome `replan`，对三条 streak 透明、不进 `COUNTED`、不推进轮次编号。

## 无头模式的红线：只提议，不落地

计划是**人和 agent 共同定稿**的东西（这一点抄 Warp，见 §0）。没人点头的时候，runner 最多只能提议：

- prompt 末尾明写：*"只输出建议清单，**不要**运行任何会改 todo 的命令（plan / edit / done 一律不要）"*；
- runner 自己**只**写一条 `replan` tick + 一份日志，**不碰 `todos`**；
- 回归测试里的假宿主就演一个"不守规矩的模型"，断言跑完之后 todo 的条数、文本、状态**一个字没变**。

## 默认开的取舍

这是唯一一个**默认开**的自动轮次（`--reflect-every` 默认关）。理由：用户要的正是"每条 todo 完成之后都看看
需不需要修正后续"，而信号门控让健康的运行**完全不付费**——顺利跑完的目标一次重估都不会触发。
不想要就 `--no-replan`。

代价是它会多一次宿主调用并打断会话谱系，所以几条**测别的东西**的 runner 测试加了 `--no-replan`
（会话谱系那条最典型：重估轮次不带 `--resume`，会混进 argv 日志）。

## 验证

`cargo test` 92 passed / 0 failed，新增 `a_headless_replan_round_suggests_but_never_edits_the_plan`：
假宿主在重估轮次里**试图**改计划 → 跑完之后 todo 条数/文本/状态全没变 / 建议进了账本和日志 /
重估次数不超过干活轮次 / prompt 里有「触发的信号」「最小改动」「不要运行任何会改 todo 的命令」/
`--no-replan` 之后一条 `replan` tick 都没有。

---

# 把回路接进每轮协议（t4）

文档同步的时候发现一个**功能性缺口**：`done` 会打出「计划可能要调整」，但**每轮协议里没说看到它该干什么**——
提示只是一行文字，agent 完全可以视而不见。自适应于是只在无头 runner 里真的发生，交互轮次里不会。

补进 `prompt.rs` 的 PROTOCOL 第 3 条（写回那一步）：

> 写回的输出里出现「计划可能要调整」时，跑一次 `zloop replan`，按它说的提最小改动并讲给用户听；**改 todo 要用户点头**，别自己动。

这条同时定死了交互侧的边界，和无头侧一致：**模型可以提议，落地要人点头。**

## 顺带同步的四处

| 位置 | 改了什么 |
|---|---|
| 概念表 | 新增 **outcome** 一行，把九种结果分成「干活的三种（算轮次算配额）」和其余六种，各自给了链接 |
| `.zloop/` 目录树 | `NOTES.md` 改成「约定 + 经验」，补上 `NOTES.md.bak-*` |
| `stats` 的指标表 | 「轮次」的口径补上 `reflect` / `replan` 也不算 |
| runner 参数表 | 补 `--no-replan` |

命令表、命令详解、runner 参数、设计文档四处都核过一遍（`zloop --help` 里 26 条命令逐条对）。

`cargo test` 92 passed / 0 failed。
