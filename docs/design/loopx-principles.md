# loopx 的思路：长时运行、上下文管理与多 agent / 多宿主协同的原理

> 目的：回答两个问题——① loopx 如何让 AI agent 长时间运行而不中断，跨轮次的"上下文"怎么管；② 多个 agent、以及 Claude Code / Codex 等多个宿主，它是怎么统一管理的。
> 依据：loopx 0.5.2 源码（本机 pip 包，≈32 万行）+ GitHub 仓库 `docs/`（architecture.md、state-interaction-model.md、quota-allocation.md、heartbeat-automation-prompt.md、concepts/field-derived-patterns.md、reference/protocols/peer-agent-runtime-v1.md、architecture/rfcs/agent-loop-effect-interpreter-v0.md、product/vision.md、guides/multi-agent-product-recipe.md）。文中 `file:line` 均指向包内源码。
> 与本仓库其他文档的关系：`loopx-scheduling-notes.md` 讲"代码长什么样、哪些可砍"；本文讲"作者为什么这么设计"。
> 记录日期：2026-08-27。

---

## 0. 一句话与总图

loopx 对"长时运行"问题的回答可以压成一句话：

> **不要让模型的对话上下文承担记忆，把一切该记住的东西外化成可重读的文件；每一轮都是无状态的、有界的、必须经过准入和结算的片段；循环本身交给宿主去跑。**

对"多 agent / 多宿主"的回答是同一句话的推论：

> **既然真源在文件而不在会话里，那么谁来执行下一轮（哪个 agent、哪个宿主）就无关紧要——只要它读同一份状态、过同一道守卫、写回同一个账本。**

作者自己的框架（`docs/architecture.md`）是"六层持久控制面 + 一个效果解释器"：

```
            ┌──────────────────────── 六层持久控制面 ────────────────────────┐
            │ 1 Registry   谁是 goal、在哪个 repo、用什么 adapter、有哪些 guard │
            │ 2 Goal state ACTIVE_GOAL_STATE.md：目标、当前判断、todo、next action│
            │ 3 Run log    每次 run 的 JSON+MD 私有证据                          │
            │ 4 Run history runs/index.jsonl：紧凑事件账本（work/decision/     │
            │               accounting/evidence 四类事件）                       │
            │ 5 Status/Attention queue  "谁该动了"的首屏投影                     │
            │ 6 Compute quota  每个 goal 能吃多少自动算力                        │
            └──────────────────────────────────────────────────────────────────┘
                                   ▲ 读                      │ 写（只有校验过的写回才算）
                                   │                         ▼
   model ──effect request──▶ harness 解释效果（quota should-run → interaction_contract）──observation──▶ model
```

作者把这叫 **"Agent Loop 是 effectful program"**（RFC `agent-loop-effect-interpreter-v0`，2026-08-08 Accepted）：模型发出"我想做 X"的效果请求，harness（loopx）决定这个效果**能不能做、怎么做、做完怎么记账**，然后把观察结果还给下一步的模型。loopx 只做中间两步，不做模型的事，也不做宿主的事。

四个角色的边界（`docs/state-interaction-model.md` "Actor Boundaries"）：

| 角色 | 拥有 | 不得拥有 |
|---|---|---|
| **Goal**（持久工作对象） | 目标、当前状态、权威来源、守卫、run 历史、下一步交接条件 | —（它就是真源） |
| **Executor**（Codex/Claude 会话） | **一轮**有界转移：读状态 → 选最高安全 lane → 做/看 → 验证 → 写回 → 交付后才记账 | 持久真相、隐式批准、未记录的奖励、**隐藏的长期记忆**、静默取消 |
| **User**（操作者/奖励源） | 边界决策、奖励、私有材料、凭证、破坏性 git、生产操作、方向变更 | 例行的公共读、拆 todo、状态写回、在已授权的 P1/P2 里挑活 |
| **loopx CLI** | 目标真相的投影、在等谁、配额、交互模式、机器义务、下一条命令 | 人的判断、私有证据解读、把项目特定分支写进 prompt |

其中最能体现立场的两条原话：

- *"A goal is not a chat thread. A thread can execute a goal, but the goal must survive thread reloads, network interruptions, and multiple project agents."*
- *"The executor is ephemeral. It should not be the source of truth."*

以及 Invariants 第一条：*"The active goal state is the durable context; chat is only execution context."*

---

## 1. 长时运行与上下文管理

### 1.1 作者的问题定义

`state-interaction-model.md` 开头把失败模式说得很清楚：**长任务失败不是因为 prompt 写得不够细，而是"下一个转移该谁负责"不明确**——agent 会等一个从未启动的线程、为小事反复问用户、因为顶层 lane 被堵就停掉整个自动化、或者在毫无变化的监控项上白烧一轮。

所以 loopx 的解法不是"更长的 prompt / 更大的上下文窗口"，而是**每轮开始先让 CLI 用确定性规则回答一个问题：现在这一轮，该谁动、动什么、动完怎么记**。这个回答就是 `quota should-run` 输出的 `interaction_contract`。

### 1.2 一轮（turn）的生命周期

```
                    heartbeat / 手动 tick / 宿主循环唤醒
                                   │
                                   ▼
Registered ──▶ Ready ──▶ QuotaCheck ──┬──▶ UserGate      需要人决定 → 提一个具体问题然后等
                                      ├──▶ AwaitEvidence 外部证据（CI/eval）未到 → 只读轮询，不发明工作
                                      ├──▶ Running       eligible + 有可执行 todo → 一个有界片段
                                      ├──▶ QuietNoop     审计后无可做 → 不花配额，自动化保持
                                      └──▶ Repair        投影过期/边界漂移 → 先修状态
                                                 │
                              Running ──▶ Writeback（产物 + 验证）──▶ refresh-state + spend ──▶ Ready
                                                                            └──▶ Done（目标终态）
```
（来源：`state-interaction-model.md` "Operational Control Loop" 的状态图；代码对应 `control_plane/quota/should_run*.py` 与 `interaction_contract.py`，见 [`loopx-scheduling-notes.md` §1 簇 A](../../docs/design/loopx-scheduling-notes.md#1-簇-a配额--should-run-引擎)、[§3 簇 D](../../docs/design/loopx-scheduling-notes.md#3-簇-d交互契约引导事务与难用根因)。）

作者给"长时执行"的操作定义（"Long-Running Todo Execution"）是**一串紧凑转移，而不是"接着上次继续"的无界循环**：

1. 跑 `quota should-run`；
2. 先服从 `interaction_contract`；
3. 允许干活时，从 `agent_todos` + 优先级栈 + 当前阻塞里选**一条** lane；
4. 顶层 P0 被堵：记录/提示阻塞，只有契约允许 safe bypass 时才去做可验证的 P1/P2，否则保持自动化但不花钱；
5. 先验证、写持久状态，再花配额；
6. **恰好花一次**——在交付、阻塞写回或实质转移之后；
7. 花完刷新状态。

### 1.3 上下文分层：谁记什么

loopx 把"上下文"拆成了明确分层的存储，每层有 owner / reader / writer（`state-interaction-model.md` "State Stores"）：

| 存储 | 内容 | 写入者 | 读取者 | 角色 |
|---|---|---|---|---|
| 项目 registry `.loopx/registry.json` | goal 身份、repo、adapter、权威来源、guards、quota 策略 | connect/bootstrap | CLI、executor、status | 身份与策略 |
| 活动状态 `ACTIVE_GOAL_STATE.md` | 目标、当前信念、todo、Next Action、验证面 | 有资格的 peer 或操作者 | executor、adapter、人 | **持久上下文（工作台）** |
| 全局 registry `~/.codex/loopx/registry.global.json` | 各项目 goal 的净化副本 | connect/refresh/sync | status、dashboard | 多项目发现 |
| Run payload `runs/<ts>.json|.md` | 单次 run 的丰富私有证据 | adapter、refresh-state | executor、本地审阅 | 私有证据 |
| 紧凑 run index `runs/index.jsonl` | 公共安全时间线 + 最新状态 | adapter、reward writer | status、dashboard、heartbeat | **append-only 事件账本** |
| 配额 / 花费账本 | `quota_slot_spent` 事件 | quota 命令、控制器 | status、automations | 算力策略 |
| Status export | 面向机器的契约 | `loopx status` | dashboard、pre-tick、heartbeat | 投影 |
| Dashboard UI state | 过滤器、选中项 | 浏览器 | 用户 | **不是** goal 真相 |

配套的两个关键约定：

- **事件账本契约**："Chat threads, browser filters, and local tool outputs may help a worker decide what to do in the moment, but they are not the durable source of truth."事件分四类：work（refresh-state、adapter tick）、decision（gate、approval、human_reward）、accounting（quota spend）、evidence（eval、CI、blocker）。当前状态是这些事件 + 活动状态 + registry 策略的**投影**；投影可以为 prompt 压缩旧细节，但不得改写让决策可审计的事件。
- **Current-Belief TODO**（`concepts/field-derived-patterns.md` §2）：ACTIVE_GOAL_STATE.md 不是流水账，它要回答三个问题——我们现在相信什么、为什么、下一个有界动作是什么。历史要压进归档（`## Completed Work Archive`，done 超 12 条即归档），"a state-only update is not a log entry; it is a new current-belief surface for the next agent tick"。

### 1.4 上下文预算：四档 prompt

`docs/heartbeat-automation-prompt.md` 明确把喂给宿主的文本分成两层、四档：

| 层 | 档 | 谁看 | 内容 |
|---|---|---|---|
| 可见目标文本 | — | 人 | 一句话，如"按 ACTIVE_GOAL_STATE.md，基于 LoopX 体系，推进项目" |
| 自动化任务体 | `--full` | 审计 | 完整生命周期协议 |
| | `--compact` | Codex App automation（上下文压力大时） | guard/gate/blocker-push/writeback/spend 规则内联 |
| | `--brief` | 已经很重的 automation | 只留身份、preflight、quota guard、硬规则 |
| | `--thin`（**默认**） | 可信 agent | **不粘贴任何命令分支**；每轮让 agent 自己去拉 registry / 状态 / history；"CLI payload remains the runtime source of truth" |

设计意图原话："This makes the Codex thread a replaceable worker and leaves durable task truth in LoopX." 以及 "Do not hand-edit per-project lifecycle branches into one automation prompt. Project-specific behavior belongs in the LoopX registry, active-state sections, adapter output…"——**项目特有的知识进状态文件，不进 prompt**；prompt 只是 bootstrap。

三者冲突时的优先级也写死了："trust the current CLI `interaction_contract` first, then use the skill as the operation manual and the prompt only as a bootstrap."

### 1.5 不空转、不中断的守卫（作者层）

`quota-allocation.md` 把"该不该跑"定义成一个**稳定的门序**（Gate Order，`field-derived-patterns.md` §7 亦同）：

1. 健康与公私边界；2. 操作者门 / 控制器 opt-in；3. 证据就绪；4. 焦点等待（focus_wait：lane 饱和、缺新颖性、等 baseline）；5. **算力配额**；6. 执行。

配额只回答一个问题："Out of the available automatic agent time, how much should this goal be allowed to consume?"——`compute ∈ [0,1]` 是占空比，`0` 是 goal 级硬暂停。它**不是**第二套权限系统："Quota does not become a second permission system."

七个产品状态：`eligible / focus_wait / throttled / waiting / operator_gate / paused / blocked_health`。

"不空转"的产品口味（作者原话）："do not let the agent idle when safe work exists, do not let it spin when no verified transition is available, and do not make the human rediscover the important gate from chat history."

### 1.6 代码层：一轮的边界到底在哪

- **开始**：宿主调度器触发 → 模型先跑守卫 `loopx quota should-run --goal-id … --agent-id … --turn-instance-id "$LOOPX_TURN"`。`LOOPX_TURN=<当前时间 ISO>` 每次触发生成、重试时复用（`control_plane/heartbeat/task_body.py:103-106`），守卫据此持久化一份幂等收据 `(goal, agent, turn_id)`（`heartbeat_receipt.py:23-38,136-192`）。should-run 是**确定性、不调 LLM**的决策器——作者在 `claude_goal_mode/README.md:3` 明说这是它拒绝用宿主 `/goal` 自判完成的原因。
- **轮内可见**：① heartbeat task_body（thin ≤1900 字符，只有"指令 + 命令"，**不含任何状态**）；② should-run JSON（20–30KB：`interaction_contract / execution_obligation / effective_action / scheduler_hint / goal_boundary / selected_todo / first_open_items(3)`）；③ 模型自己去读的 `ACTIVE_GOAL_STATE.md`、`status --limit 3`、`review-packet --handoff-only`（`task_body.py:327-329`）。
- **结束**：验证 → `refresh-state --classification … --delivery-batch-scale … --delivery-outcome …`（写 run 记录）→ `quota spend-slot --slots 1` **恰好一次、失败不重试**（`task_body.py:196-227`）→ 按 `user_channel.notify` 决定向用户输出还是静默。

**四种宿主下，模型的对话上下文是复用还是丢弃**——这是理解"长时运行"的关键：

| 宿主 | 一轮怎么开 | 对话上下文 |
|---|---|---|
| Claude Code `/loop` | 原生 `/loop` 每 tick 执行 `.claude/loop.md`：`should_run → claim_task → 做一段 → complete_task(evidence) → should_run`（`goalmode_cmd.py:56-78`）；MCP 工具只是 shell 出 CLI（`goal_mode_mcp.py:110-215`） | 同一 Claude 会话内**复用**（宿主所有），loopx 不注入也不依赖 |
| Codex App automation | 用 task_body 建 heartbeat automation，初始 3 分钟 RRULE（`host_loop_activation.py:700-726`） | 每 tick **新会话、丢弃**。设计语（`:723`）："next wakeup starts from LoopX quota/status/state, **not stale chat memory**" |
| Codex CLI `/goal` | `/goal <task_body>` 设一次；"Reuse this Goal until terminal; do not create a successor host Goal"（`task_body.py:576-579`） | 同一 TUI Goal **复用**；连续 3 次相同 blocked 才 `update_goal status=blocked`（`rules.py:63-68`） |
| headless `turn run-once` | TurnEnvelope（≤8192B，`quota/turn_envelope.py:23`）经 stdin 喂给 `codex exec … --output-schema`（`turn_driver/codex_cli.py:249-266,335-381`） | 按 **(goal, agent, todo) 谱系**复用：`goals/<goal>/turn-sessions/<sha256(lineage)>.json` 存 `session_id`，存在则 `codex exec resume`（`:65-80,402-411`）；**换 todo 即新会话**；schema 被拒/会话丢失时丢弃重开（`:38-44,493`）；超时先保存会话供 `resume_session` 恢复（`:481-491`） |

也就是说：loopx **从不主动管理模型的对话上下文**。宿主愿意复用就复用，不复用也无所谓——因为设计上每一轮都必须能从文件冷启动。

### 1.7 代码层：跨轮记忆有哪几层

| 通道 | 写入者 → 读取者 | 上限 / 生命周期 |
|---|---|---|
| `ACTIVE_GOAL_STATE.md` 各 section：Objective / Authority Sources / Operating Contract / Execution Profile / Non-Goals / User Todo / Agent Todo / Next Action / Recent User Feedback / Progress Ledger / Completed Work Archive（`bootstrap.py:480-540`） | 模型 + CLI（todo add/complete；refresh-state 写 Next Action `state_refresh.py:268-290`；feedback 写 Progress Ledger `feedback.py:316-333`） | done > 12 条归档进 Archive 段（`todos/completed_archive.py:15-16`）；文件级永久 |
| todo 元数据字段 `note / evidence / reason / completed_at / completion_turn_key / validation_command / result_hash / consecutive_no_change`（`todos/contract.py:1145-1160,1280-1300`） | complete / monitor writeback → should-run 投影 | 投影时每字段截 180 字符（`quota_summary.py:50`） |
| `~/.codex/loopx/goals/<goal>/runs/<ts>.json+.md` + `index.jsonl`（`history.py:97-116`），每条含状态快照：next_action(8 行) / recent_feedback(5) / progress(5) / sha256（`state_refresh.py:485-515`） | refresh-state / spend / monitor-poll → `collect_history` 全量重读、`latest_runs` 截断、`semantic_history` 按 agent 只留 **6 个"最新语义槽"**（`run_context_retention.py:10-17,105-178`） | append-only，永久 |
| `rollout-event-log.jsonl`（`rollout_event_log.py:15-40`） | quota_should_run 收据、todo 事件 → heartbeat receipt / evidence log | 幂等 event_id |
| `turns/<turn_key>.json` journal | run-once 各阶段 checkpoint → 崩溃重放 | 每轮一份 |
| `turn-sessions/<sha>.json` | codex_cli host → 下轮 resume | 按 todo 谱系 |
| agent_turn_recall 收据 `.local/loopx/agent-turn-recall/<id>.json` | 可选 CLI，把 reward-memory 召回注入"私有上下文"（`capabilities/agent_turn_recall/core.py:208-296`） | **默认关闭**，fail-open |
| OpenViking | 项目 = peer，`ov find` ≤3 次召回偏好（`extensions/openviking_semantic_preference/provider.py:20,255-271`）；run 结论导出为公共安全 markdown 供另行 ingest（`history_export.py:1-15`） | **默认关闭** |
| dreaming | 离线读最近 ≤50 条 run 分类归纳为 refactor / memory_consolidation / archive / exploration 提案（`dreaming.py:82-107,244-337`），仅 advisory，approve 后才变 todo（`:410-470`） | 不写状态 |

注意最后三行：**语义层面的"经验记忆"在 loopx 里是可选、默认关闭、fail-open 的**。默认配置下，跨轮传递的只有结构化字段（分类、哈希、截断文本）。

### 1.8 代码层：预算怎么控、停滞怎么测、崩了怎么恢复

**预算是分层而非截断**：`INTERFACE_BUDGET_CHARS = full 12000 / compact 6200 / brief 3500 / thin 1900 / visible_goal 4000`（`heartbeat/budget.py:8-14`）。thin 由独立模板 + 压缩规则常量渲染（`task_body.py:661-703`，`rules.py:21-57`），`build_interface_budget` 只**度量** `within_budget`（`budget.py:40-66`），唯一硬失败是 native goal host > 4000 抛错（`builder.py:382-393`）。层级：thin prompt 1.9K → should-run JSON（热路径预算 small 20K / crowded 30K / multi-agent 23K，`testing/cli_output_budget.py:124-150`）→ 冷路径 `status / history / --include-detail`（55K+）→ headless TurnEnvelope 8192B → handoff 16 行 / 1800 字符，超预算按段落逐级删（`handoff_budget.py:6-10`，`review_packet.py:76-99`）。为什么 should-run 允许 20–30K：它是**唯一决策包**，用来取代模型自己读全量状态。

**不烧额度**：spend 每轮一次，skip / 预检失败 / dry-run 不 spend；unchanged 轮询上限 `cli_limit = claude_limit = 3` → 本地调度停 / Claude loop 停 / TUI 退出，停之前做一次最终 replan 检查（`scheduler_hint.py:498-505,1064-1152`）；Codex App RRULE 指数回退 3 → 60 分钟（`:54,487-489`）。

**outcome floor**：`surface_only` 连续 ≥3 → `focus_wait`，仅允许 `outcome_floor_recovery` 旁路（`execution_profile.py:18-30`，`quota.py:208-273`）；小步连续 ≥2 → cadence hint `thin_progress / widen | replan`（`long_task_cadence.py:143-168`）。

**无进展检测**（全部是 typed 指纹，**不看散文**）：`progress_observation` 指纹（result_class + surface / hypothesis / probe / evidence ids）连续 2 次相同 → `typed_progress_repeat`（`progress_observation.py:224-278`）；monitor `result_hash` 不变则 `consecutive_no_change+1`（`monitor_poll_writeback.py:247-256`），≥5 触发（`autonomous_replan_obligation.py:242`）；todo 链 ≥15 advancement / ≥20 open 触发（`long_todo_chain.py:17-18`）。

**处置**：生成 `autonomous_replan_obligation`（todo_actions: split / add / retire / ask_decision + stop_condition，`autonomous_replan_obligation.py:520-716`），**只能被 typed semantic delta 关闭**（new_surface / new_hypothesis / new_concrete_blocker / coverage_backed_*），散文不算（`progress_observation.py:298-360`）；ack 绑定 `frontier_revision`（`long_todo_chain.py:247-290`）。loop controller 六态 `run_now | wait | user_action_required | repair | replan | terminal`，预算耗尽 → replan（`loop_controller.py:53-60,581-586`）。

**崩溃恢复**：journal 分阶段 `host_execute → typed_result → validation → durable_writeback → quota_spend → terminal_closeout`（`effect_program.py:92-96`），带前值 sha256 的原子写（`turn_journal_runtime.py:85-119`）；committed / stopped 直接重放（`executor.py:1385-1396`），failed + retry 从失败阶段续（`:1397-1421`）；`resume_session` 仅限 host_execute 阶段失败（`session_recovery.py:47-50`）；`state_backup` 打包 runtime root + 各项目 `.loopx / .codex/goals / .claude/goals`（`state_backup.py:152-156`）。

### 1.9 立场的三个代码证据，以及代价

> **记忆不在模型脑子里，在文件里；每轮是有界、可重放、由确定性控制面守门的无状态片段。**

1. `host_loop_activation.py:723`："not stale chat memory"。
2. 进度只承认 typed 字段："deliberately not used to classify prose"（`progress_observation.py:24-27`）；"Legacy ACK … never count"（`:427-437`）。
3. headless host 明令模型 "Do not write LoopX state, spend quota… the adapter owns those effects"（`codex_cli.py:257`）——模型只产出 typed result，效果由 harness 解释。

**代价**：

1. **每轮全量重读**：`collect_history` 逐行读整份 `index.jsonl` 再投影（`history.py:163-312`），should-run 20–30K/tick；thin prompt 只是把 token 从 prompt 挪到了工具输出。
2. **语义有损**：run 快照仅 5 条进度 / 5 条反馈、文本 180–360 字符、vision ≤3200（`executor.py:60`）；推理轨迹不保留，只剩分类与哈希。
3. **依赖模型自律**：spend 一次不重试、"never upgrade to multi_surface"等全靠 prompt 约束；仅 Claude `--harden` 钩子与 run-once 有强制，交互式 `/goal` 与 Codex App 没有。
4. **停滞检测需要模型主动写 typed observation 与稳定 id**，否则退化为周期评审；无效行直接 `None`（`progress_observation.py:193-201`）。
5. **会话复用粒度是 todo 而非 goal**，换 todo 即冷启动；115s 超时（`codex_cli.py:392`）；TS effect runtime 任何 shape 不匹配即 `RuntimeError` fail-closed——用可用性换正确性。
6. **长期"经验"层实质缺席**：OpenViking / reward memory / turn recall 默认关闭、fail-open；dreaming 只产提案。

---

## 2. 多 agent 协作管理

### 2.1 作者的立场：对等 peer，不是主从

`reference/protocols/peer-agent-runtime-v1.md`：

> "`peer_v1` removes durable agent rank from LoopX runtime decisions. Registered agents have equal identity authority. Work ownership comes from todo claims, task leases, explicit continuation policy, and bounded task-scoped assignment."

规范身份只有这么多：

```json
{"schema_version": "peer_agent_identity_v1", "agent_model": "peer_v1",
 "agent_id": "codex-alpha", "registered": true, "registered_agents": ["codex-alpha", "codex-beta"]}
```

并且**禁止**输出 `primary_agent` / `handoff_agent` / 带等级的 `role` 字段（连 null 占位都不行）。这是从 v0.1 的"主控 + 侧路"（main-control / side-bypass）层级模型迁移过来的，迁移模块被隔离在 `legacy_migration`，运行时不再有主从分支。

### 2.2 工作归属的五条规则（协议原文）

1. 显式的 `claimed_by` 或活跃的 task lease 获胜；
2. 未认领的 todo 必须先 claim 或 lease 才能交付；
3. 明确指派给某 agent 的 replan 义务留给它；
4. 未指派的 replan 义务**用哈希**（canonical work key × 排序后的 registered_agents）确定性地分给恰好一个 peer；
5. 注册顺序不得影响确定性分配。

"The deterministic assignment is coordination for one work item. It does not change identity authority and must not be persisted as an agent rank."

### 2.3 交接是 task 策略，不是身份策略

- `continuation_policy=independent_handoff`：后继 todo 留空，任何合格 peer 都可拿（除非显式选人）；
- `continuation_policy=same_agent_non_delivery`：后继留给完成者本人；
- **Review 只是一个 `action_kind`**，不是特殊的交接类型——要"作者以外的任何人来审"，就用普通的 independent_handoff 并把作者加进 `excluded_agents`；
- 合并权限归仓库维护者策略，peer 身份既不给也不剥夺 self-merge。

`field-derived-patterns.md` §8 定义了"交接包"应包含什么：goal id 与当前分类、一条推荐动作、看过的文件/面、权威来源、验证面、硬守卫、残余风险、**允许进入下一阶段的确切条件**；不含原始私有证据，也不假装是用户批准。

### 2.4 并行的前提：显式认领 + 不相交写范围 + 工作区隔离

`field-derived-patterns.md` §9："parallelism only works when claims are explicit"——registered agents 平等；claim/lease 分配只读探索 / 实现 / 验证；**write scopes 必须不相交**；临时的 task coordinator 可以收集 bundle 证据，但 merge / 发布 / 生产权限仍归仓库策略与操作者门。

`peer-agent-runtime-v1.md` "Workspace Isolation"：`agent_workspace_guard_v1` 要求——只要选中的 todo 声明了写范围、或 action_kind 属于写类、或 goal 策略要求隔离——**每个 peer 必须用独立的 git worktree**；只读观察与监控不触发。

`architecture.md` 对 lease 的路线图说得很实在：`claimed_by` 是**软**归属（"for visibility only"，写在活动状态文件锁内，仅接受已注册 id）；硬 lease（`task_lease_v0`：owner、TTL、write scope、幂等键、冲突、transfer、release）只在"宿主已证明存在并发写问题"时采用；**争用单位是 `(goal_id, todo_id)`，不是整个 goal**——同一 goal 下不同 todo 只要写范围与门允许就能并行。

### 2.5 协调靠状态文件，不靠消息

`guides/multi-agent-product-recipe.md`："The preset should not directly call another agent, inject text into another pane, or keep private side-channel state. **Agents coordinate by reading and writing the shared LoopX state surface**: registry, runtime root, todo projection, quota/frontier, run history, and public-safe evidence."

多 agent 可见运行的三层结构：

| 层 | 拥有 | 不得拥有 |
|---|---|---|
| 用户层 | 主题、目标、轮数、可选角色覆盖 | tmux、Codex 启动参数、pane 内 tick 命令 |
| 产品预设 | 角色表（`agent_id / role_id / lane_id / scope`）、每角色 skill 片段、交接提示、种子 todo、证据适配器 | 通用 runner、真实 TUI pane、A2A tick、todo/证据/状态协议 |
| 多 agent 内核 | runner、真实交互式 Codex TUI pane、pane 本地 A2A tick、工作区可信启动、todo/证据/状态协议、紧凑人类状态 | 任何领域语义 |

一次 `loopx multi-agent launch --spec … --execute --attach` 打开一个 tmux 会话，每个 pane 是一个**真实的、可以被人打断并直接对话的** Codex CLI agent（"The pane is a real Codex CLI agent, not a passive log viewer"）；每个角色的第一个动作是 pane 本地的 A2A tick（即跑一次 should-run）；"todos and evidence are the only handoff authority"。

监督者（supervisor）是对等模型之上的**可选覆盖层**，"not a replacement for it"（`peer-supervisor-v0`）。

### 2.6 目标层级与跨项目

registry 里的 `spawn_policy`（是否允许 spawn 子 goal、`max_children`、允许的 domain）、`parent_goal_id`、`role=controller`、`coordination.write_scope`、`requires_parent_approval=[write, publish, production-action]` 构成父子 goal 的边界；全局 registry 是各项目 registry 的**同步副本**（"agents should not manually paste project entries into a separate queue"），`loopx global-summary / global-todos / global-gates / global-risks` 给一个人类管理者看多个项目——这是 `product/vision.md` 里"maintainer-first management surface"的落点：**第一屏回答"agent 做了什么、正在做什么、哪里卡住、接下来会发生什么、需要我什么、我的反馈会怎样改变计划"**。

### 2.7 代码层：身份是什么粒度

- **粒度 = 一条 "agent lane"**：一个公共安全字符串 id（`^[a-z][a-z0-9_.:@-]{0,79}$`，`control_plane/todos/contract.py:27`），按 goal 登记在 `coordination.registered_agents`（`agent_registry.py:24-40`）。**它不是进程也不是人**：多个宿主进程可以共用一个 id，所以 lease 另用 `idempotency_key` 标识"执行实例"（`work_items/task_lease.py:270-275`）。新宿主会话默认注册新 id，接管旧 lane 需显式意图（`host_loop_activation.py:619-633`；`register-agent --require-new` 撞名即拒）。
- **peer_v1 是唯一运行模型**（`control_plane/agents/runtime_model.py:15-28`）：v0.1 的 primary / side-agent 层级被迁移删除（`legacy_migration.py:16,105-136`）；`profile_role` **禁止** leader / manager / supervisor / worker 字样（`profile.py:27-66`）。profile 只是建议性路由（preferred / avoid action_kind），仅在同一 claim 桶内调序（`claim_visibility.py:86-91`）。
- **should-run 缺 `--agent-id` 为什么拒绝**：roster 存在却无身份 → `blocks_should_run=True`（`agents/identity.py:80-104`）；给了却未注册 → 抛错（`:38-47`）。原因：无身份的心跳会拿走任何 todo，claim / lease / authority 全部无法归因（`mutation_authority.py:221-226`）。
- **thread binding**：`(host_surface, thread_id) → agent_id` 存于 `coordination.thread_agent_bindings`（`thread_agent_binding.py:50-75`），解析出 bound / missing / conflict（`:78-112`），已绑他人则拒绝、需显式 unbind（`:269-281`）。解决的是"同一 Codex / Claude 线程下次 `/loopx` 回到同一 lane，而不是按 registry 顺序猜"。

### 2.8 代码层：这一轮我该拿哪条 todo

**claim 是软的**：`todo claim` 把 `claimed_by=<id>` 写进 ACTIVE_GOAL_STATE.md 的注释元数据（`todos.py:1327-1332`）；已被他人 claim 则拒（`mutation_authority.py:248-260`）。

**lease 是硬的、可选的**：独立 JSON `runtime_root/goals/<g>/task-leases/<todo>.json`（`task_lease.py:101-110`），TTL 默认 45 分钟、上限 24 小时（`:38-39`）；acquire 要求 owner 已注册、不在 excluded、与 claimed_by 不冲突、todo open（`:632-670`），且 `write_scopes` 与其他活跃 lease **不重叠**（`:909-953`）；renew / transfer / release 都要 `expected_version` CAS；transfer 必须换新 key、epoch+1（`:1207-1282`）。**过期只是失效，没有自动再分配**（`:831-837`）。

**handoff_mode**（写在活动状态 front-matter）：`legacy` = 软 claim + 硬 lease 并存（**作者承认这是脑裂**，`handoff_mode.py:5-8,20-32`）；`soft_claim` 禁 lease；`hard_lease` 改 owner 须持活跃 lease、完成须过 lease fence（`task_lease.py:172-243,258-471`）。

三个容易混淆的词：**agent lane** = 身份通道；**work lane** = 本轮契约 `advancement_task | continuous_monitor`，到期的 monitor 可抢占（`work_lane.py:76-87`）；**scope** = `goal_all_read_claimed_run_global_read_v0`（`agent_scope.py:89`）——全读、只跑自己 claim 或未 claim 的；**frontier** = 本 agent"无候选"时的类型化裁决 `agent_scope_exhausted | agent_scope_wait | reassignment_required | successor_replan_required`（`agent_scope_frontier.py:11-16`）。

goal 级义务（replan、临时协调者）用哈希确定性指派：`select_peer_for_work = sha256(work_key) % n`（`runtime_model.py:63-73`）。

```python
def pick_todo(me, todos, gates, leases, mode):
    # 用户门只阻塞被点名的人：global_gate 或 blocks_agent==me 或 claimed_by==me   agent_scope.py:282-328
    if any(g.open and (g.global_gate or g.blocks_agent == me) for g in gates):
        return WAIT_GATE
    cand = [t for t in todos if t.open and t.task_class == "advancement_task"
            and me not in t.excluded_agents                       # todos.py:246-268
            and t.claimed_by in (None, me)]
    if due_monitor_preempts(...):  return MONITOR_LANE             # work_lane.py:76-87
    cand.sort(key=(0 if t.claimed_by == me else 1, profile_rank, priority))   # claim_visibility.py:53-72
    if not cand:                   return frontier_rule_chain(me)  # wait / reassign / exhausted / replan
    t = cand[0]
    if t.claimed_by is None:       emit("todo claim --claimed-by me")          # agent_scope.py:707-724
    if mode == "hard_lease":       require(lease(t).owner == me and active)    # task_lease.py:172-243
    elif lease(t).active:          require(my idempotency_key + version)       # task_lease.py:461-471
    return t
```

`continuation_policy`：`independent_handoff`（默认）后继 todo 不带 claim；`same_agent_non_delivery` 后继自动 claim 给完成者（`completion_policy.py:165-170`）。

### 2.9 代码层：互斥、传播与交接

**互斥**：状态文件靠 flock——兄弟 `.lock` 文件 `LOCK_EX|LOCK_NB` 轮询，MUTATION 5s / MONITOR 1s / SINGLE_FLIGHT 0s（`file_lock.py:86-102,378-418`），超时抛 `lock_acquire_timeout` 并留 holder / incident 记录；锁内 read → 改行 → `write_text`（**无乐观哈希**）；锁序固定：先 state 锁再 lease 锁（`handoff_mode.py:136-139`）。

**mutation_authority**（`mutation_authority.py:176-299`）：≤1 个注册 agent 走兼容模式（不校验）；否则必须 `--agent-id`；actor ∉ excluded_agents；user todo 的 bound_agent / blocks_agent 必须等于 actor；改他人 claim 的 todo 需 `coordination.todo_lifecycle_authority` 授权 → `delegated_orchestration_override`。

**传播**：所有 agent 读同一份 markdown。`user_gate` 带 `blocks_agent=X` 只阻塞 X（`agent_scope.py:293-303`），`global_gate=true` 阻塞全员（`:240-242`），多 agent 下必须二选一显式声明（`write_policy.py:42-68`）；他人的 gate 在我的视图里是 `other_agent_scoped_items`，不阻塞我。

**交接没有消息通道，交接 = 写 todo 元数据**：① `todo complete --next-agent-todo [--next-claimed-by | --next-excluded-agent]` 生成后继并以 `unblocks_todo_id / successor_todo_ids` 链接（`todos.py:1922-1975`）；② `todo update --claimed-by` 转归属；③ `task-lease transfer`；④ blocker：`status=blocked/deferred + resume_when: todo_done:|pr_merged:|capacity_available:`；⑤ user gate：`--role user --task-class user_gate --blocks-agent | --global-gate`，完成时 `decision_outcome approve|reject|cancel` 精确解锁。传递的内容是只读投影 `handoff_note`：from / to_agent、intent、summary、evidence_refs、unresolved_decisions、blocked_on、suggested_next_action（≤280 字、凭证正则清洗，`handoff_note.py:19-27,117-190`）。

### 2.10 代码层：监督与"集体节奏"

- **supervisor 是戴帽子的 peer**（`configure-goal --supervisor-agent`）：不能监督自己，`execution_mode=proposal_only`、`peer_authority=equal_identity_authority`（`supervisor.py:60-104,121-180`）。读 attention_queue、`agent_management_projection`（含 `stale_claim_hint`，阈值 36h，`management_projection.py:408-447`）、evidence log；**只能产出 observe / inject / handoff / discard 提案**，执行须外部 host 回执（`supervisor_events.py:153-162`）；事件 append 到 `goals/<g>/supervisor-events.jsonl`。`supervisor_inject` 是没有默认实现的 adapter 缝（`supervisor_inject.py:39`）。
- **collective round** = 每条 lane 各得一次 quota / frontier / turn 机会（`collective_round_ledger.py:304-308`），纯函数账本，"completed" 需"有选中且已执行"。
- **visible_wake_scheduler**：每 2 秒子进程跑 `loopx todo list`，为每 lane 算一个 token（可运行 todo id 的排序元组），token 变化才唤醒（`visible_wake_scheduler.py:27-96`）；载荷是固定提示词经 tmux `set-buffer / paste-buffer / Enter` 敲进 pane（`visible_multi_agent_tmux.py:277-380`）；**广播者不选 todo、不写状态、不花 quota**（`multi_agent/contract.py:196-206`）——"LoopX state, not the scheduler, decides the work"（`:22-29`）。
- **tmux 布局**：一个 session、tiled 多 pane，每 pane 一个交互式 Codex TUI（**仅支持 Codex**），注入 `LOOPX_GOAL_ID / LOOPX_AGENT_ID`（`visible_multi_agent_launcher.py:235-245`）；pane 内 `loopx-pane-a2a-tick` 先 `quota should-run --agent-id` 再 frontier / worker（`runtime_scripts.py:260-384`）。**botmux ≠ tmux**：它是飞书 bot daemon 的 HTTP 客户端，按状态文件哈希去重派发"跑下一轮"（`botmux_runtime.py:26,840-871`）。

### 2.11 代码层：目标层级与跨项目

- `role: controller | subagent` 与 `parent_goal_id` **仅声明性**（`registry.py:424-425` 无授权分支）。`spawn_policy{mode, allowed, max_children, allowed_domains}` 是子 agent 编排闸门（`task_orchestration.py:276-297`），准入拒因 `task_domain_not_allowed / capacity_deferred / write_scope_conflict`。
- `coordination.write_scope` 是**真约束**：todo 声明的 `required_write_scopes` 越界 → 本轮降级为 `boundary_projection_repair`（`projection_repair.py:185-215`），临时扩权走 `checkpointed_boundary_authority`（`boundary_authority.py:173-198`）。`requires_parent_approval=[write, publish, production-action]` 硬编码、只投影不拦截（`goal_boundary.py:178-186`）。
- 全局 registry：项目主动 `sync-global` 注册（`global_registry.py:623-796`），路由冲突抛错需 `--replace-state`；`global-todos` 分 runnable / deferred_ready / blocked / review（`global_todos.py:189-301`）；`global-gates` 区分 user gate（有 user_gate todo 实体、可精确解锁）与 controller gate（由 waiting_on 推出、阻塞整 goal）（`summary_all.py:263-312`）。**lease / lock 事实不出项目**——跨项目视图看不到互斥状态。

### 2.12 立场的代码证据，以及代价

> **对等 peer 而非主从；以 todo 行为最小交接单位；靠共享状态文件 + 锁 / 租约协调；调度器只敲门不决策。**

- `runtime_model.py:15-28` + `profile.py:27-66` 禁层级角色；
- `multi_agent/contract.py:22-29`："LoopX state, not the scheduler, decides the work"；
- `supervisor.py:103,135`：`proposal_only / equal_identity_authority`；
- `handoff_note.py:126-128`："read model, no dispatcher queue"。

**代价**：

1. **claim / lease 双轨脑裂**被作者明文承认且默认保留（`handoff_mode.py:5-8,20-32`），front-matter 跨主机 last-writer-wins。
2. **无自动接管**：lease 过期 / claim 陈旧只在 36 小时后产生一条提示"请同一 agent 恢复"（`management_projection.py:446`），frontier 让别人"安静等待"（`agent_scope.py:1487-1500`）——**一个 agent 挂掉，它认领的活会饿死**，需人或授权门介入。
3. 一致性全靠**本机 flock + 2 秒轮询子进程**，`write_text` 非原子；跨主机同步不在契约内。
4. **身份无认证**：能写文件系统者可冒任何 agent_id；write_scope 依赖 todo 自我声明，属荣誉制 + 修复轮。
5. 表面积大（`--agent-id / --claimed-by / --blocks-agent / --bound-agent / --global-gate / --goal-bound / --excluded-agent / --idempotency-key / --expected-version`……），可见多 agent 仅支持 Codex TUI，supervisor 注入无内置 adapter。

---

## 3. 跨宿主（Claude Code / Codex / 其他）统一管理

### 3.1 宿主不是一个接口类，而是一份"激活包"

loopx 没有 `Host` 基类。每个宿主是 `host_loop_activation.py` 里一个 `_xxx_activation()` 函数，返回同构的 dict：`activation_method`、`host_mutation.{owner, host_command|host_tool, cli_can_mutate_directly, missing_host_tool_gate}`、`activation_steps`、`success_criteria`（`host_loop_activation.py:700-1213`，分派表 `:1254-1288`）。

**真正统一的接口是 CLI 契约**：任何宿主每轮都从 `quota should-run` 进入、拿 `heartbeat-prompt` 的 `task_body`（`control_plane/heartbeat/builder.py:85-106` 按 profile 选 thin / visible_goal / ark 渲染器）、以 `turn_envelope`（`quota/turn_envelope.py:22`）或 `loopx_turn_result_v0` 回执。

宿主 × 能力点对照：

| 宿主（runtime_profile） | 触发下一轮 | 喂 prompt | 回执 | 强制拦截 | 安装入口 |
|---|---|---|---|---|---|
| **Codex App**（`codex_app_heartbeat`） | 原生 automation RRULE | task_body 作 automation 正文 | `scheduler-ack` + heartbeat receipt（`turn_instance_id`，`host.py:93-100`） | 不支持（`:709`） | `~/.codex/skills/$loopx`（Codex 无自定义 slash `:972-982`） |
| **Codex CLI / IDE / SSH**（`codex_cli` / `codex_app_ssh_goal`） | 原生 `/goal <task_body>`（≤4000 字，`budget.py:13`） | /goal 正文 | 3 次 unchanged → `update_goal blocked`，loopx 侧仍 active（`rules.py:63-68`） | prompt-only；另有 headless `turn run-once --host codex-cli` 内建 runner（`turn_driver/codex_cli.py:384`） | skills + `codex-cli-bootstrap-message` |
| **Claude Code**（`claude_code`） | 原生 `/loop`（老一代跑 `.claude/loop.md`，`goalmode_cmd.py:56-77`） | loop.md：should_run → claim → do → complete | MCP `complete_task`；`claude_code_loop` 无变化 3 次 → stop（`scheduler_hint.py:594-599`） | 可选 `--harden` PreToolUse fail-closed（`goal_policy.py:176-198`），Bash 仅黑名单 | 新一代仅 SKILL.md；老一代 `claude mcp add` + commands + statusline |
| **OpenCode 1**（`generic_cli`） | 插件监听 idle + timer | `loopx_goal_activate(goalId, objective)`（`:858-865`） | `--record-host-poll` receipt | 原生（bridge 不注入即停），无工具级拦截 | `--surface opencode --with-goal-bridge` 写 `plugins/loopx-goal.js` |
| **OpenCode 2**（`generic_cli`） | loopx 进程外 worker 持 timer，经 HTTP API 驱动 | worker `--task-body` | receipt + pid lock | 原生；`cli_can_mutate_directly=True`（`:982`） | 宿主侧无需安装 |
| **Pi**（`generic_cli`） | 扩展 `agent_settled` → should-run → `sendUserMessage(followUp)`（`loopx-goal.ts:12-15,240`） | 同上 | receipt；Esc 持久化 `autoResume:false` | 原生 | `--surface pi` |
| **Gemini / Cursor**（`generic_cli`） | **无循环原语**，模型自己每轮跑 should-run（`:1043-1051`） | skill 文本 | 无保证 | 不支持 | `GEMINI_HOME/skills`；Cursor 合并 `mcp.json` |
| **DSH**（`generic_cli`，outer_controller） | loopx `turn run-once` | stdin 签名 envelope 的 `primary_action`（`turn_host_adapter.py:84-110`） | stdout 一个 typed result，loopx 独立验证后才写 | 原生（loopx 持循环与 spend） | `loopx[deepseek-harness]` |
| **KunlunCode**（`kunluncode`） | loopx 外层控制器经 app-server `thread/goal/set strict` | controller 提交 | `verification_passed` 后才 refresh / complete / spend | 原生 + 写保护：MCP claim/complete 与 CLI 写 fail-closed（`guards.py:27-111`） | `loopx-kunluncode install/connect` |
| **Ark**（`ark_managed_agent_goal`） | 宿主 Goal runtime 自持续 | 一次性 `submit_goal_once` | `goal_runtime_continuation{continue_now, defer, complete}`（`execution_context.py:431-467`） | 不支持；`phase_handoff_allowed:False` | `workflow-skills --host-surface ark-managed-agent` |

### 3.2 四套命名轴、两层设计

- **`--host-surface` / agent_type**（连字符，17 个产品身份，`host_loop_activation.py:60-78`）：回答"我在哪个 UI"。歧义值 `codex` 直接拒绝（`:277-282`）。
- **`SchedulerRuntimeProfile`**（8 个，`execution_context.py:38-46`）：`--runtime-profile` 的快捷别名，展开为三元组（`:77-118`）；多对一——opencode / pi / gemini / cursor / dsh / traex 全是 `generic_cli`。
- **`HostSurface × SchedulerOwner × ExecutionMode`**（8×5×3，`:13-35`）：scheduler_hint 真正推理的坐标；`_validation_errors`（`:183-242`）枚举合法组合，非法则要求 `repair_scheduler_execution_context`。
- **Turn host kind** `{codex-cli, claude-code, generic-cli}`（`driver.py:20`）：进程边界。

为什么要两层：profile 给 hook / MCP 一个稳定短参数；`scheduler_owner`（谁叫下一轮）与 `execution_mode`（跑在可见交互 / 隔离无头 / 托管自动化哪种进程里）才是决策维度——同一个 `generic_cli` 既可以是 `agent_cli_loop`（Pi 自己跑）也可以是 `outer_controller`（DSH 由 loopx 跑）。

### 3.3 "谁拥有循环"的五种模式

| `scheduler_owner` | 宿主 | 回执 | 退避 / 停止 |
|---|---|---|---|
| `host_automation` | Codex App；`local_scheduler`（launchd tick，`codex_cli_scheduler.py:254-269`，只输出候选命令不执行） | `quota scheduler-ack --applied-rrule --reset-token --identity-signature`（`scheduler_ack.py:140-191`） | 指数退避（`scheduler_hint.py:486-498`）；`reset_token` 变则回初始 RRULE；stop → `automation_update` 暂停；fallback `loopx-apply-rrule` 直改 SQLite |
| `agent_cli_loop` | Codex `/goal`、Claude `/loop`、OpenCode / Pi / Gemini / Cursor | `--record-host-poll`；Claude 用 `complete_task` | `unchanged_poll`：codex_cli_tui → exit/blocked、claude_code_loop → stop、其余 `stop_tick_loop`；终局前 `final_quota_replan_check` 再跑一次（`:500-508`） |
| `goal_runtime` | Ark | `continuation.defer` 带 `recheck_after_seconds` | loopx 不包 Turn |
| `outer_controller` | DSH、OpenCode2 worker、Kunlun、`turn run-once` | typed result + journal（turn_key） | loop_controller 预算、worker lock、`--controller-timeout-secs` |
| `none` | 手动 | 自报 | 无 |

### 3.4 同一 goal 能否跨宿主接力

**条件**：① 同一项目 registry——所有适配器经 `goal_mode_context.find_registry` 向上找 `.loopx/registry.json`（`:14-28`）；② 同 goal_id + 同一 agent lane：`coordination.registered_agents` 是跨宿主共享的身份池（`agent_registry.py:24-40`）；thread binding 按 `(host_surface, thread_id)` 索引（`thread_agent_binding.py:98-112`），换宿主后 binding 缺失 → `thread_binding_selection_required` 门（`host_loop_activation.py:597-617`），需显式 `--agent-id`（接管）或 `--new-peer`；只有一条 lane 时自动选中（`:661-671`）。

**明确限制**：

- `route_collision`：同 goal_id 若 `source_registry / repo / state_file` 变化即拒（`global_registry.py:202-231`）。
- Turn session 含 host 且按 `(goal, agent, todo)` 哈希（`codex_cli.py:65-80`），`identity_mismatch` 拒绝（`driver.py:181-189`）。
- host_loop_activation **不持久化**，registry 只加性标注 `agent_backends`；"agent type changed → 重跑 agent-onboard"（`agent_onboarding.py:490-497`）。
- scheduler 状态按 surface 分桶，换 profile 会自动剥掉 `codex_app_*` 键（`execution_context.py:534-546`），**但不会通知旧宿主停**——Codex App 的 automation 会继续打，需要在旧宿主侧执行 stop hint。

所以答案是：**能接力，靠的是"同一份文件 + 同一个身份池"，而不是任何会话迁移机制**；代价是旧宿主的循环要人手停。

### 3.5 统一入口 `/loopx` 如何做到一份模板多宿主

- 一份 spec `_command_prompt_specs`（`slash_command_install.py:200-297`），唯一占位符 `$ARGUMENTS`；按 surface 只换外壳，Claude / Gemini / Cursor / OpenCode 共用同一 SKILL.md 正文，仅根目录不同（`:1032-1036`）。
- **host 检测交给模型而不是 CLI**（`:209`）：CLI 进程分不出 Codex App / CLI / IDE（同一个 `codex` 二进制、同一个 CODEX_HOME），能嗅探的只有 `CODEX_THREAD_ID` 与 `LOOPX_ENTRY_HOST_SURFACE`；作者选择"宁 gate 不猜"（`host_mode_planner.py:268-291`）——不确定就返回 16 条 rerun 命令的选择门。
- managed 标记 `<!-- loopx-managed-slash-command:v1 command=… surface=… -->` 写进 md / js / ts，`_target_status` 判 created / updated / unchanged / upgraded_legacy / skipped_user_file（`:390-409`），卸载只删带标记文件。
- readback `.loopx-skill-install.json` 记 owner / integration_mode / source revision / 每技能树 sha256（`skill_install_readback.py:317-357`），解决"宿主加载的到底是不是 loopx 装的、有没有被改、版本是否与 CLI 一致"。

### 3.6 立场与证据

> **loopx 不拥有循环，只拥有每轮的准入与结算；宿主用自己的原语跑循环，接不上就明说是 prompt-only。**

- `_skill_facade_cli_activation` 的 docstring："weaker guarantee… stated as such, because claiming autonomous heartbeat support these hosts cannot deliver is worse"（`host_loop_activation.py:1043-1051`）。
- 17 个宿主里 `cli_can_mutate_directly` 仅 kunluncode / opencode2 为 True（`:901, :982`），其余都附 `missing_host_tool_gate` 文本。
- `claude_goal_mode/README.md:3-6` 弃用宿主自带的 `/goal`："it judges completion from the transcript, which conflicts with LoopX's deterministic gate"——**完成与否必须由确定性守卫判断，不能由模型看对话记录自判**。

### 3.7 代价与局限

1. **加一个宿主要改 ≥6 处 if/elif 表**（host_loop_activation 6 处、bootstrap_command_pack 2 处、`_command_prompt_specs` 手写 host 列表、agent_onboarding 3 个函数、slash_command_install 一个分支），没有插件注册机制。
2. **prompt-only 宿主不可强制**：Gemini / Cursor / 默认 Claude / Codex `/goal` 全靠模型自觉；硬拦截只有 Claude `--harden`（Bash 黑名单）和 Kunlun 的环境变量守卫（unset 即失效）。
3. **Codex 泄漏进核心**：每个 scheduler_hint 都带 `codex_app` 键（非 Codex 写 `not_applicable`）；`DEFAULT_PERMISSION_RULE` 写死 "Codex session"（`rules.py:5`）；`worker_bridge.py:26` 写死 `/opt/homebrew/bin/codex`；Turn 只有 codex-cli 有内建 runner，`--host claude-code` 只能 plan。
4. **四套命名值不一致**（`codex_app_ssh_goal`、`codex-cli-tui` vs `codex_cli`），靠 `HOST_SURFACE_TO_AGENT_TYPE`、`VISIBLE_*_ALIASES` 和 legacy flag 回退粘合。
5. **循环核心被重写 4 次**：`goal-bridge-runtime.mjs` 29KB、`opencode2-goal-worker.mjs` 35KB、`pi-goal-loop-runtime.mjs` 21KB、`kunluncode/runtime.py` 644 行，各自实现 backoff / lock / receipt，策略源在 Python，漂移没有契约约束；host poll receipt 按 goal 单文件不分宿主（`host_poll_receipts.py:34-42`），双宿主同跑会互相覆盖。

## 4. 总评

### 4.1 三条主线其实是一个想法

| 问题 | loopx 的回答 | 关键机制 |
|---|---|---|
| 怎么长时间运行不中断 | 不靠模型记忆，靠**可重读的文件 + 每轮无状态冷启动 + 确定性守卫** | ACTIVE_GOAL_STATE.md（current-belief）、runs/index.jsonl（事件账本）、`quota should-run`（唯一决策包）、spend-after-validation、typed 停滞检测、journal 重放 |
| 上下文怎么管 | **分层外化**：prompt 只留 1.9K 的 bootstrap，状态给 CLI 投影（20–30K），细节冷路径按需拉 | 四档 prompt 预算、should-run 热路径预算、语义槽 6 个、快照 5 行、180 字符截断 |
| 多 agent 怎么协作 | **对等 peer，todo 是最小交接单位，共享文件是唯一协调通道**，无消息、无主从 | registered_agents、claimed_by（软）/ task lease（硬）、excluded_agents、continuation_policy、user gate（blocks_agent / global_gate）、supervisor 只提案 |
| 多宿主怎么统一 | **loopx 不拥有循环**，只拥有每轮准入与结算；宿主用自己的原语跑循环 | host_loop_activation 激活包、runtime_profile → (surface × owner × mode) 三元组、`/loopx` 一份模板多宿主、readback 校验 |

把四行合起来就是 §0 那句话：**真源在文件，不在会话；每轮都是可以从文件冷启动的有界片段；因此谁执行、在哪执行都不重要。** 多 agent 和多宿主不是额外设计出来的能力，而是"状态外化"这个决定的自然推论——这也是为什么 loopx 的代码里多 agent 的部分与单 agent 共用几乎全部路径，只多了身份 / claim / gate 这几个字段。

### 4.2 这套思路真正新的地方

1. **把"该不该继续"从模型手里拿走**。宿主自带的 `/goal`、`/loop` 都是让模型看对话记录自判"做完了没"；loopx 坚持这必须是确定性、不调 LLM 的函数，并为此弃用了宿主的 `/goal`。
2. **进度必须是 typed 的**。散文写"有进展"不算，必须有新 surface / 新 hypothesis / 新 concrete blocker 的结构化字段，否则触发 replan 义务。这是对"agent 假装忙碌"最直接的防御。
3. **spend after validated writeback**。配额不在"准备跑"时扣，在"做完且写回"后扣——一个跑了但没产出的 turn 不计入账本，从会计口径上就否认了空转。
4. **把 human-in-the-loop 缩小到边界决策**。用户只管门（gate）、奖励、方向；不管路由、不管拆 todo、不管每一步批准。"do not make the human rediscover the important gate from chat history"。

### 4.3 这套思路的边界

- **它假设模型足够自律**去执行写回协议。硬强制只存在于 Claude `--harden` 钩子和 headless run-once；Codex App / Codex `/goal` / Gemini / Cursor 全靠 prompt。当模型不写回，控制面只能事后靠 typed 指纹发现"没进展"。
- **它用大量结构化字段换取可审计性**，代价是每轮 20–30K 的 JSON、约 50 个 todo 元数据字段、497 种 schema_version——对单人单机场景是纯税（见 [`loopx-scheduling-notes.md` §3.3](../../docs/design/loopx-scheduling-notes.md#33-难用的-9-条结构性根因有证据)）。
- **语义记忆缺席**。跨轮传的是分类和哈希，不是"上次为什么这么判断"；OpenViking / reward memory 是可选外挂。长期项目里"教训"的沉淀依赖模型主动把它写进 ACTIVE_GOAL_STATE.md 的 Operating Contract 或 todo note。
- **多 agent 的容错是人肉的**。没有自动接管、没有心跳超时再分配，一个 agent 消失，它认领的 todo 要等人来解。

### 4.4 对 zloop 的启示（一句话版）

zloop 保留了 §4.1 表里每一行的"回答"，只删掉了为多 agent / 多宿主 / 可审计性付出的字段与仪式：单 JSON 真源、`next` 是纯函数、`done` 合并写回与记账、prompt ≤1200 字符、宿主只需一份 SKILL.md。它**没有**继承的是 typed 进度指纹（用 `--outcome progress|fail` + fail_streak 粗略替代）和 claim / lease（明确不做多 agent）。如果将来 zloop 需要多 agent，按 loopx 的经验，**先加 `claimed_by` 软归属 + `excluded_agents`，不要一上来做 lease**——作者自己的路线图也是这么写的。
