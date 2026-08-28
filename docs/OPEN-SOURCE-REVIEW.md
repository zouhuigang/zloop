# 开源方案对照：zloop 还能做得更好的地方

> 范围：除 loopx 之外，2026-08 时点上与"让 coding agent 长时间围着一个目标干活"直接相关的公开方案。每个方案只提炼与 zloop 相关的机制，逐条对照 zloop 0.2 现状，给出借鉴项与优先级。
> 日期：2026-08-27。来源见文末。

## 0. 结论先行

| 方案 | 它最值得学的一点 | zloop 现状 | 借鉴项 |
|---|---|---|---|
| **Anthropic《Effective harnesses for long-running agents》** | 把"完成"外化成带测试步骤、初始 `passes:false` 的 feature list；每 session 开头有固定的"找回方位"流程；一次只做一个 feature；每 session 一次 git commit | todo 只有文本，没有验收标准；`context` 相当于 progress 文件；没有 git checkpoint | **P0** todo 可带验收标准，`done` 前对照；P1 `--git-commit`、可选 preflight 命令 |
| **Ralph Wiggum loop（含 Claude Code 官方插件）** | Stop hook 把"想停"变成"再来一轮"；`--max-iterations` 是唯一真正的安全网；把"卡住时怎么退出"写进 prompt | zloop 的 Stop hook 是同一思路，但比它多了结构化状态；`--max-rounds`、fail/progress streak 相当于安全网 | **P0（bug）**：runner 拉起的 `claude -p` 会触发我们自己的 Stop hook，一轮变多轮——必须让 hook 识别 runner 环境并放行 |
| **Beads（bd）/ Gas Town** | `bd ready --json` 只给"可领的活"；依赖图有 blocks / parent-child / discovered-from；旧任务"记忆衰减"压缩；`bd remember` 存跨 session 的经验 | `blocked_by` 只有 blocks 一种；`next --peek` ≈ ready 但只给一条；无压缩；无经验记忆 | P1 `remember`（经验进 `context`）、`compact`（归档旧 tick/todo）；P2 `parent-child` |
| **OpenHands** | 三道闸缺一不可：`MAX_ITERATIONS`、`LLM_NUM_RETRIES`、累计成本硬顶；context condenser 让成本从二次方变线性 | 有轮数闸和 streak 闸，**没有成本闸**；会话按 todo 谱系相当于粗粒度 condenser | **P0** 每轮记 `total_cost_usd`，policy `max_total_usd` 作为第三道闸 |
| **Claude Code 原生 `/goal` + `/loop until:` + Cloud Routines** | 宿主自己就有"目标 + 直到条件满足"的循环；`/loop max: 20`；Routines 云端跑 | zloop 的价值不在循环本身，而在**跨宿主的文件状态、留档、会话回看、runner**；应明确定位而不是重复造循环 | P1 `zloop pause` / `resume`（对齐 Codex `/goal pause|resume`，现在要手改 JSON）；P2 README 写清与原生 `/goal` 的关系 |
| **Codex `/goal`（0.128+，默认开启）** | 持久化 goal、`pause / resume / edit / clear`；"goal 改变的是持久性，不是权限" | 同上 | 同上 |
| **通用：等人时怎么叫人** | 上述所有方案都假设人会自己回来看。Ralph 靶 iterations 停、Anthropic 靠人开新 session、Codex "stop and ask for help" | zloop runner 等人时慢速轮询，但**没人知道它在等** | **P0** 等人 / 停机 / 连续失败时主动通知（飞书 webhook 或任意命令） |

## 1. Anthropic：Effective harnesses for long-running agents

**机制**（原文摘录）：

- 两个 agent：*initializer* 建环境——`init.sh`、`claude-progress.txt`、首个 git commit、`feature_list.json`；*coding agent* 每 session 只推进一个 feature。
- `feature_list.json` 每项 `{category, description, steps[], passes: false}`；"a 'clone of claude.ai' project … over 200 features … all initially marked with a passes: false status"。JSON 比 Markdown 稳："the model was less likely to inappropriately modify or delete JSON entries"；coding agent **只允许改 `passes`**："It is unacceptable to remove or edit tests".
- session 开头固定流程：`pwd` → 读 git log 与 progress 文件 → 从 feature list 选最高优先级未完成项 → 跑 `init.sh` → 做一次端到端基线验证 → 再动手。
- 结束前：git commit（描述性 message）、更新 progress 文件、"leave the codebase in a mergeable state"。
- 观察到的失败与对策：过早宣布完成 → 穷尽的 feature list；上下文中途耗尽留下半成品 → 一次一个 feature + 频繁 commit；标了 passes 但端到端不工作 → 强制用浏览器自动化自测后才能标 passes。

**zloop 对照**：todo = feature，`done` = passes 翻 true，`context` ≈ progress 文件，一轮一条 = one feature at a time，`.zloop/log` ≈ 每 session 的记录。**缺**：① todo 没有 `steps`/验收标准，`done` 只凭模型一句 note；② 没有 session 开头的"找回方位"仪式（zloop 的 heartbeat 第 1 条只是 `context` + `next`，没有"跑一遍基线验证"）；③ 没有 git checkpoint。

**借鉴**：
- **P0 验收标准**：todo 增加可选 `acceptance`（`plan` 行语法 `[P0] 文本 :: 验收标准`，`edit --acceptance`）；`next --json` 带出；heartbeat 协议第 2 条改为"做完先对照验收标准自检"；`done` 时有 acceptance 却没 `--evidence` → 打印提醒（不阻断，避免把模型逼成空话）。
- **P1 preflight**：policy `preflight_cmd`（如 `./init.sh` / `cargo test`），runner 每轮先跑，结果摘要放进 prompt；失败就不开这轮、记 `fail`。
- **P1 git checkpoint**：`run --git-commit`：每轮写回后若仓库 dirty 则 `git add -A && git commit -m "zloop <todo>: <note>"`，默认关。

## 2. Ralph Wiggum loop

**机制**：一个 `while true` 反复喂同一份 prompt；官方插件用 Stop hook 在同一会话内实现——"The prompt never changes between iterations / Claude's previous work persists in files / Each iteration sees modified files and git history"。`--completion-promise "<text>"` 精确匹配一段输出算完成；"Always rely on `--max-iterations` as your primary safety mechanism"。建议把"15 轮还没完成就写下阻塞点、尝试过什么、建议替代方案"写进 prompt。

**zloop 对照**：zloop 的 Stop hook（`hook-stop`）与 Ralph 同源，但 Ralph 的状态只在文件和 git 里、完成靠一句魔法字串；zloop 有结构化 todo、`done` 是显式完成信号、fail/progress streak 相当于 Ralph 建议的"卡住就写下阻塞点"。

**发现的 bug（P0）**：Claude Code 文档明确 `claude -p` "loads the same context an interactive session would, including anything configured in the working directory or `~/.claude`"，也就是**会执行我们装的 Stop hook**。runner 拉起 `claude -p` 做一条 todo，模型 `done` 后想结束，hook 看到还有可执行 todo → 阻止退出 → 模型继续做下一条……一次 `-p` 调用被拖成多轮：runner 的"一轮一条"、`--timeout-min`、`--max-rounds`、每轮 session 记录全部失真。（`--bare` 能跳过 hooks，但 bare 模式不读订阅登录、要 API key，不适合默认。）**修法**：runner 给子进程设 `ZLOOP_RUNNER=1`，`hook-stop` 见到即放行。

## 3. Beads（bd）

**机制**：面向 agent 的分布式图状 issue tracker。issue 有 priority/type/assignee/state；依赖类型 `blocks / related-to / parent-child（bd-a3f8.1.1）/ supersedes / duplicates / discovered-from`；**hash id** 防多 agent 合并冲突；`bd ready --json` 只返回无开放阻塞的任务——"genuinely reduces token waste compared to feeding full plan documents"；**compaction**："Semantic 'memory decay' summarizes old closed tasks to save context window"；`bd prime` 注入工作流上下文与持久记忆，`bd remember "insight"` 存跨 session 知识。推荐每 session：`prime → ready → update --claim → close → remember`。

**zloop 对照**：`plan/next/done` ≈ `create/ready/close`；`context` ≈ `prime`；`blocked_by` 只有 blocks；单 agent 不需要 hash id / claim。**缺**：经验记忆（`remember`）、旧记录压缩、父子拆分。

**借鉴**：
- **P1 `zloop remember "…"`**：追加到 `.zloop/NOTES.md`，`context` 新增「经验」段（最近 N 条）。这是 Anthropic progress 文件里"lessons"那一半，zloop 目前只有"做了什么"没有"学到什么"。
- **P1 `zloop compact`**：把 done 超 N 天的 todo 与对应 tick 摘要成一行归档到 `.zloop/archive/`，state.json 保持小；`context`/`status` 不受影响。
- P2 `--parent t3`（子任务）——目前用 `done --next` 顺序展开已够。

## 4. OpenHands

**机制**：headless 模式"always-approve, blast radius = workspace"，必须 Docker；三道闸 `MAX_ITERATIONS`（默认 ~100）、`LLM_NUM_RETRIES`（8）、累计成本硬顶——"Headless agents should not be shipped without all three"；context condenser 触发后"per-turn API costs to less than half the baseline"，成功率不降（54% vs 53%）。

**zloop 对照**：轮数闸有（`max_runs`、`--max-rounds`）、重试交给宿主（`claude -p` 自带 api_retry）、**成本闸没有**——`--max-budget-usd` 只限单轮。`claude -p --output-format json` 返回 `total_cost_usd`（"so scripted callers can track spend per invocation"），我们没读。

**借鉴（P0）**：runner 把 `total_cost_usd`、`num_turns`、`duration_ms` 写进 tick（`cost_usd` 等可选字段）；policy `max_total_usd`（0 = 不限）→ 窗口内累计成本达顶 `decide` 返回 `budget` 停机；`status` 显示"本目标已花 $x.xx"。Codex 无成本字段，只记 duration。

## 5. Claude Code 原生 `/goal`、`/loop until:`、Routines；Codex `/goal`

**机制**：Claude Code 2026 春起有 `/goal <可校验的终态>`（"Claude evaluates each action against the goal and keeps working until the condition is met or it determines it can't proceed further"）、`/loop every 10m` / `/loop until: <cond>` / `/loop max: 20`、云端 Routines（"keeps working even with the laptop closed"）。Codex 0.128+ `/goal` 持久化并有 `pause / resume / edit / clear`，0.133 起默认开启；原则"A goal changes persistence, not authority"。

**对 zloop 的含义**：单会话内的"直到条件满足"循环，两个宿主都已经原生提供，zloop **不该再在这个层面竞争**。zloop 的独特价值是：① 状态在文件里、跨宿主、跨会话、跨机器重启；② 每轮留档 + 会话回看；③ runner 不依赖任一宿主 UI 存活；④ 停机条件确定性（不靠模型自判）。README 应把这层关系写清（P2）。

**借鉴（P1）**：`zloop pause` / `zloop resume`（改 `goal.status`），对齐 Codex 的心智模型，现在要手改 JSON。

## 6. 没人做好的一件事：等人时叫人

Ralph 靠 `--max-iterations` 停下、Anthropic 靠人开下一个 session、Codex 建议 agent "stop and ask for help"、loopx 有 `NOTIFY` 通道但只是"向用户输出"。都假设**人会自己回来看**。zloop 的 runner 在 `user_gate` 下慢速轮询是对的，但一个跑通宵的任务凌晨 2 点卡在"要不要替换付费 SDK？"上，早上 9 点才有人发现，7 小时白等。

**借鉴（P0）**：policy `notify_url`（POST JSON；URL 含 `feishu`/`lark` 时用飞书自定义机器人格式 `{"msg_type":"text","content":{"text":…}}`，否则 `{"text":…}`）与 `notify_cmd`（任意 shell 命令，事件 JSON 从 stdin 进）。runner 在 phase 进入 `waiting (user_gate)`、`stopped (*)`、限流退避、连续失败时触发一次（同一原因不重复）。用 `curl` 发送，零依赖。

## 7. 借鉴清单

| 优先级 | 项 | 来源 | 落地 |
|---|---|---|---|
| **P0** | runner 子进程 `ZLOOP_RUNNER=1`，`hook-stop` 放行 | §2（bug） | t2 |
| **P0** | tick 记 `cost_usd / num_turns / duration_ms`；policy `max_total_usd`；`status` 显示累计花费 | §4 | t2 |
| **P0** | todo `acceptance`（`::` 语法 / `edit --acceptance`），`next --json` 带出，heartbeat 要求自检，`done` 无 evidence 提醒 | §1 | t2 |
| **P0** | `notify_url` / `notify_cmd`：等人、停机、限流、连续失败时通知 | §6 | t2 |
| P1 | `zloop remember` + `context` 经验段 | §3 | t3 |
| P1 | `zloop pause` / `resume` | §5 | t3 |
| P1 | `run --git-commit` 每轮 checkpoint | §1 | t3 |
| P1 | policy `preflight_cmd` 每轮前自检 | §1 | t3 |
| P1 | `zloop compact` 归档旧 tick/todo | §3 | t3 |
| P2 | README：与原生 `/goal`、Ralph、Beads 的关系 | §5 | t4 |
| P2 | `--parent` 子任务 | §3 | 暂不做 |

## 8. 处置结果

### P0（t2，2026-08-27）

| 项 | 实现 | 测试 |
|---|---|---|
| Stop hook × runner 冲突 | runner 给子进程设 `ZLOOP_RUNNER=1`；`hook-stop` 见到即放行（exit 0、无输出） | `cli_test::hook_stop_passes_through_under_runner`、`runner_test::runner_records_cost_and_marks_child_env`（假宿主读到 `runner=1`） |
| 成本记录与总预算 | tick 新增 `cost_usd / num_turns / duration_ms`（claude 从 `-p` JSON 取，codex 只记时长）；结算时补进 tick 与该轮 log 文件；policy `max_total_usd`（0 不限）→ `decide` 新停机原因 `budget`；`status` 显示 `spent: $x / max $y`，`context` 目标段显示已花费 | `tick_test::budget_cap_stops_when_spent_reaches_max_total_usd`、`cli_test::status_shows_spend_and_notify_cmd_receives_events`、runner cost 测试 |
| 验收标准 | todo 新增可选 `acceptance`：`plan` 行 `文本 :: 验收`、`edit --acceptance`；`next --json` 的 todo 对象带出；`status`/`context`/log 显示；heartbeat 第 2 条要求"逐条对照自检通过才算完成"；`done` 有验收却无 `--evidence` 时打印提醒（不阻断） | `todo_test::plan_line_acceptance_syntax`、`cli_test::acceptance_shows_up_and_done_without_evidence_hints` |
| 通知 | policy `notify_url`（curl POST，URL 含 feishu/lark 用飞书机器人格式）与 `notify_cmd`（`sh -c`，事件 JSON 进 stdin，`ZLOOP_EVENT/ZLOOP_TEXT/ZLOOP_ROOT` 环境变量）；runner 在进入等人、限流、停机（除 `--max-rounds`）时发一次并去重；新命令 `zloop notify [文本]` 测试配置 | `runner_test::wait_and_stop_trigger_notifications`（等人恰好 1 次 + 停机 1 次）、`status_shows_spend_and_notify_cmd_receives_events` |

全量 `cargo test`：56 通过。

### P1（t3，2026-08-27）

| 项 | 来源 | 实现 | 测试 |
|---|---|---|---|
| `zloop remember "…"` | Beads `bd remember`、Anthropic progress notes 里的"教训" | 追加到 `.zloop/NOTES.md`；`context` 新增「经验」段（最近 5 条）；heartbeat 第 3 条提示模型用它 | `cli_test::remember_pause_resume_and_compact` |
| `zloop pause` / `resume` | Codex `/goal pause|resume` | 改 `goal.status`；runner 下次检查即 `stop (paused)` 并通知 | 同上 |
| `run --git-commit` | Anthropic：每 session 一次 commit | 写回后 `git add -A -- .` → `git reset -- .zloop` → 有暂存才 commit `zloop <todo>: <note>`；journal 记 `commit`。踩坑：pathspec 里显式写被忽略的 `.zloop` 会让 `git add` 退出 1 | `runner_test::git_commit_checkpoints_each_round_excluding_zloop_dir` |
| policy `preflight_cmd` | Anthropic：session 开头跑 `init.sh` + 基线验证 | runner 每轮前 `sh -c`（受 `--timeout-min` 约束）；失败 → 记 `fail`、不调宿主、journal `preflight_failed`；通过 → 摘要进 prompt | `runner_test::preflight_failure_records_fail_and_success_reaches_the_host` |
| `zloop compact --keep-days N` | Beads 记忆衰减 | 把 done/deferred 超 N 天的 todo 及其 tick 移到 `.zloop/archive/compact-<ts>.json` | `cli_test::remember_pause_resume_and_compact` |

全量 `cargo test`：59 通过。命令总数 19 个用户命令 + 内部 `hook-stop`（比借鉴前多了 notify / remember / pause / resume / compact 五个，每个 ≤1 个 flag）。

## 来源

- Anthropic Engineering, *Effective harnesses for long-running agents*：https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
- Ralph Wiggum plugin（anthropics/claude-code）：https://github.com/anthropics/claude-code/blob/main/plugins/ralph-wiggum/README.md ；官方插件市场：https://github.com/anthropics/claude-plugins-official/tree/main/plugins/ralph-loop
- Beads：https://github.com/gastownhall/beads/blob/main/README.md ；Steve Yegge, *The Beads Revolution*：https://steve-yegge.medium.com/the-beads-revolution-how-i-built-the-todo-system-that-ai-agents-actually-want-to-use-228a5f9be2a9
- OpenHands context condensation：https://www.openhands.dev/blog/openhands-context-condensensation-for-more-efficient-ai-agents ；SDK 论文：https://arxiv.org/html/2511.03690v1 ；headless max_iterations 修复：https://github.com/OpenHands/OpenHands/pull/6865
- Claude Code `claude -p` 文档（cost 字段、hooks 加载、SIGTERM、`--bare`）：https://code.claude.com/docs/en/headless
- Claude Code `/goal` `/loop`：https://www.mindstudio.ai/blog/claude-code-goal-loop-commands-autonomous-tasks ；Routines：https://pasqualepillitteri.it/en/news/4358/claude-code-autopilot-loop-cloud-routines
- Codex `/goal`：https://kingy.ai/ai/openai-codex-goal-the-new-long-horizon-mode-for-agentic-coding/ ；changelog：https://www.developersdigest.tech/blog/codex-changelog-april-2026
