# loopx 核心调度机制提炼笔记

> 目的：为重写精简版 `zloop` 提炼 [huangruiteng/loopx](https://github.com/huangruiteng/loopx) 中与 Claude Code / Codex 调度直接相关的机制，明确"保留什么、砍掉什么"。
> 依据：本机已安装的 `loopx 0.5.2`（pip 包即 GitHub 仓库源码）；GitHub 仓库 2026-05-31 创建，2026-08-26 仍在活跃推送，5.2k star，Apache-2.0。
> 记录日期：2026-08-26。

## 0. 规模与结构事实（难用的底层原因）

| 指标 | 数值 | 说明 |
|---|---|---|
| Python 文件数 | 819 | `site-packages/loopx` |
| Python 总行数 | 317,699 | 约 32 万行 |
| 运行时依赖 | 0 | 所有 `Requires-Dist` 都是 `extra`（test / deepseek-harness），核心是纯标准库 |
| console_scripts | 5 | `loopx` + kunluncode / lark-provider / openviking ×2 |
| 顶层子命令 | 100+ | `loopx -h` 的 choices 列表超过一屏 |
| 与调度直接相关的模块行数 | ≈ 13,000（顶层）+ ≈ 40,000（control_plane 四个子包） | 见第 1–4 节 |
| 本机 Claude Code 侧安装物 | 11 个 skill（`loopx`、`loopx-global-*`、`loop-global-*` 旧别名、`loopx-pr-review`），0 个 hook | `~/.claude/skills/loopx/SKILL.md` 只有 21 行 |
| 本机 Codex 侧安装物 | `~/.codex/loopx/`（全局 registry + run history）、`~/.codex/hooks.json`、`~/.codex/AGENTS.md` | 运行时根目录被 Claude Code 复用 |

结论：**重的不是依赖，是概念数量与代码体积**。一个"提示模型每轮该干什么"的控制面被做成了 32 万行、100+ 子命令、每条命令几十个 flag 的系统。

## 0.1 作者自己画的主干（README）

```text
objective / issue / project
   │
   ▼
LoopX state: objective + gates + todos + scope + evidence + quota
   │
   ├─ human judgment needed? ── yes ─▶ ask a concrete question and wait
   ├─ safe fallback available? ──────▶ run one bounded agent slice
   ▼
Codex / Claude Code / Cursor / shell agent executes one turn
   ▼
write evidence + handoff + next todo ─▶ quota decides the next tick
```

这张图就是 zloop 要保留的全部骨架：**状态 → 判断要不要跑 → 跑一段 → 写回 → 决定下一 tick**。其余都是旁路。

## 0.2 磁盘上的真实状态形态（本项目 bootstrap 后实拍）

### `.loopx/registry.json`（项目级）

```jsonc
{
  "schema_version": "0.1",
  "updated_at": "…",
  "common_runtime_root": "/Users/<me>/.codex/loopx",
  "goals": [{
    "id": "zloop-goal",
    "domain": "project-goal-control-plane",
    "status": "active",
    "role": "controller",
    "repo": "/abs/path",
    "state_file": ".codex/goals/zloop-goal/ACTIVE_GOAL_STATE.md",
    "adapter": {"kind": "read_only_project_map_v0", "status": "connected-read-only"},
    "spawn_policy": {...},            // 多 agent 旁路
    "coordination": {"registered_agents": ["claude-code-zloop-a1"], "agent_model": "peer_v1", ...},
    "execution_profile": {            // 大段"节奏契约"文本，多数只是提示
      "cadence": "bounded_progress_segment",
      "spend_rule": "spend_only_after_artifact_validation_writeback",
      "must_include": ["coherent_artifact","targeted_validation","state_writeback"],
      "outcome_floor": {...}, "degradation_policy": {...}
    },
    "next_probe": "loopx … check …",
    "guards": ["read-only by default", ...]
  }]
}
```

一个 goal 真正需要的字段其实只有：`id`、`objective`、`status`、`repo`、`state_file`。其他都是策略/多 agent/适配器元数据。

### `ACTIVE_GOAL_STATE.md`（真正的活动状态）

- YAML front-matter：`status / owner_mode / objective / updated_at / adapter_id`
- 人读 section：Objective、Authority Sources、Operating Contract、Execution Profile、Non-Goals、Recent User Feedback、Progress Ledger
- 机读 section：`## User Todo`、`## Agent Todo`、`## Next Action`
- **Todo 行语法**（机读核心）：

```markdown
- [ ] [P0] 任务文本
  <!-- loopx:todo todo_id=todo_6b30b12daf8d status=open task_class=advancement_task action_kind=study_reference claimed_by=claude-code-zloop-a1 updated_at=2026-08-26T22:54:38%2B08:00 -->
- [x] [P1] 已完成任务
  <!-- loopx:todo todo_id=… status=done … completion_continuation=no_followup no_followup=true note=<urlencoded> evidence=<urlencoded> completed_at=… completion_turn_key=local_completion_<hash> -->
```

  - 复选框 = 完成态；`[P0]/[P1]/[P2]` 在文本里；所有元数据塞进 HTML 注释，值做 URL 编码（中文 note/evidence 完全不可读）。
  - `## Next Action` 下再放一份文本副本 + `<!-- loopx:next-action … todo_id=… -->`。

评价：**把机器状态藏进 Markdown 注释**是双输——人看不懂 `%E6%96%B0…`，机器要先解析 Markdown 再解析注释再 URL 解码。zloop 应改为 **JSON 是唯一真源，Markdown 只是只读投影（可选）**。

## 0.3 本次 `/loopx` 引导流程实测（Claude Code 宿主）

`/loopx <goal>` → skill 让模型执行 `loopx start-goal --guided --host-surface claude-code` → 返回 8 步事务：

1. `inspect_connection`（只读）
2. `connect_if_needed` → `loopx bootstrap …`（写 registry + state）
3. `select_agent_identity` → `loopx register-agent --require-new --execute`（必须先注册身份，再**重跑一次** start-goal）
4. `plan_ranked_todos`（模型规划 P0/P1/P2）
5. `write_ordered_todos` → 逐条 `loopx todo add …`（每条 7 个必填 flag）
6. `refresh_state`
7. `activate_host_loop` → `loopx heartbeat-prompt --thin …`（Claude Code 只能拿到一段 prompt，真正的循环得用户自己敲 `/loop`）
8. `quota_guard` → `loopx quota should-run …` → 返回 `decision=run` + `interaction_contract.cli_channel.next_cli_actions`（两条命令：refresh-state、spend-slot）

实测这一套跑下来：**12 条 CLI 调用、3 个 JSON 包（每个 30KB+，objective 文本在包里重复出现 21 次——包自己的 `duplication_measurement` 字段承认了这一点）**，才到"可以开始干活"。zloop 的目标是把这 8 步压成 **1 条命令**。

---

## 1. 簇 A：配额 / should-run 引擎

### 1.1 决策流程（从输入到输出）

读取的状态（`cli_commands/quota.py:172-318` → `collect_status`）：

- **registry**：项目 `.loopx/registry.json` 同步到 `~/.codex/loopx/registry.global.json`，取 `goal.status`、`goal.quota{compute,window_hours,slot_minutes,allowed_slots}`、`execution_profile`。
- **run history**：`~/.codex/loopx/goals/<goal>/runs/index.jsonl`（`history.py:237-260`），统计已花 slot。
- **ACTIVE_GOAL_STATE.md**：解析 User/Agent Todo → attention item（`control_plane/todos/todo_summary.py:179-258`）：有 open user todo → `waiting_on=controller`；有 open agent todo → `waiting_on=codex`。
- 最新 run 的 `classification` → `waiting_on`（`work_items/attention_routing.py:118-190`）。

quota 字段计算（`quota.py:294-374`）：

```
compute       = clamp(goal.quota.compute, 0..1, default 1.0)
window_hours  = goal.quota.window_hours  or 24
slot_minutes  = goal.quota.slot_minutes  or 1
allowed_slots = goal.quota.allowed_slots or round(window_hours*60/slot_minutes*compute)   # 默认 1440
spent_slots   = Σ event.slots  for run in index.jsonl
                where now-window ≤ run.generated_at ≤ now and classification=="quota_slot_spent"
              − Σ voided（quota_slot_voided）
```

`goal.quota.spent_slots` 字段实际被忽略，只信 runtime 事件。

状态梯（`quota.py:376-440`）：

```
if goal stopped or compute<=0                    -> paused
elif severity=="high"                            -> blocked_health
elif waiting_on in {user_or_controller,controller} -> operator_gate
elif waiting_on=="external_evidence"             -> waiting
elif waiting_on=="codex" and lifecycle in {continuation_boundary,focus_wait} -> focus_wait
elif waiting_on=="codex":  spent>=allowed ? throttled : eligible
else                                             -> waiting
# 叠加 outcome floor（quota.py:208-292）：连续 3 次"表面进展" 且 eligible -> focus_wait + safe_bypass_kind=outcome_floor_recovery
```

决策核心（`should_run.py` / `should_run_prepare.py` / `decision_summary.py` / `should_run_packet.py:1063-1085`）：

```
item = build_quota_plan(status).items[goal_id]
if not item:                    return decision="skip"
if item.quota.state=="paused":  return skip, scheduler=stop
health_ok = 无 error_diagnostics
normal    = health_ok and state=="eligible"
recovery  = health_ok and safe_bypass_kind=="outcome_floor_recovery"
self_repair / capability_repair / workspace_repair = 各类旁路守卫
should_run = normal or recovery or self_repair or capability_repair or workspace_repair
decision = replan_obligation ? "autonomous_replan_required"
         : normal   ? "run"
         : recovery ? "safe_bypass_recovery"
         : self_repair ? "self_repair" : capability ? "repair_bridge" : workspace ? "workspace_guard"
         : "skip"
```

`decision` 全集有 11 个值（`run / observe / safe_bypass_recovery / self_repair / repair_bridge / workspace_guard / automation_prompt_upgrade / autonomous_replan_required / successor_replan_required / peer_coordination_blocked / skip`）。**但真正让 agent 干活的 run 条件只有一条：`health_ok ∧ state==eligible`**，即 `waiting_on==codex ∧ spent<allowed ∧ 无 gate`。

### 1.2 `spend-slot`

- `quota.py:1115` → 先跑一次 should-run 作为 `before`，要求 `before.ok` 且放行；模拟 `spent+slots` 得 `after`。
- `--execute` 写 3 处：`runs/<ts>-quota-slot-spent.json`、同名 `.md`、`runs/index.jsonl` 追加一行 `classification=quota_slot_spent`。**registry 永不改写**。
- `spend_after_validation` 语义：should-run 只"放行"不扣费；agent 干完活、用 refresh-state 写回带 `delivery_outcome` 的 run 后，再 spend-slot 记账。幂等键 = `turn_instance_id/todo_id`，重复调用返回 `idempotent_replay`。`void-slot` 可反向冲销。

### 1.3 `execution_profile` 到底控制什么

`cadence / minimum_scale / must_include / spend_rule` 全是**纯标签**：只被拼成提示文本进 prompt（`delivery_contract.py:72-142`），没有任何分支读它们的值。真正影响决策的只有两个数字：

- `outcome_floor.surface_streak_threshold`(3)：连续 3 次表面进展 → eligible 降为 focus_wait。
- `degradation_policy.small_scale_streak_threshold`(2)：只喂提示文本，不改 should_run。

### 1.4 调度节奏（下次多久醒）

- `long_task_cadence.py:110-183`：根据最近 runs 的 `delivery_outcome/turn_kind/batch_scale` 算 streak，输出 `{signal: blocked|thin_progress|material_progress, recommendation: wait|keep|widen|replan}`。**不算时间，只是提示**。
- `scheduler_hint.py:857-1218` 才算分钟数：由最终 `interaction_contract` 推 disposition → 区间 `[i, 2i, 4i]`（Codex App 硬顶 60 分钟）：

| disposition | 分钟 | 无变化轮询上限 |
|---|---|---|
| paused/stopped/terminal | 停 | — |
| active_work | 3 → 10 | 无 |
| human_gate | 30 → 120 | 3 |
| quiet_wait（throttled 等） | 30 → 120 | 3 |
| monitor_wait | 15, 30, 60 | 3 |
| unchanged_wait | 60 → 240 | 3 |

连续 3 次无变化 → 最后再跑一次 should-run，仍无变化则停。`reset_token = digest(action+identity+profile)` 变化即回到首区间。Codex App 的 backoff 下标持久化在 `<runtime_root>/scheduler-state/<hash>.json`，由 `quota scheduler-ack` 写入。`heartbeat_prequota.py` 只做 PR review 对账，恒返回 `continue_to_quota=True`——可整块删。

### 1.5 磁盘形态（`~/.codex/loopx/`）

```
registry.global.json                        # 全局 registry（各项目 .loopx/registry.json 同步而来）
goals/<goal_id>/runs/index.jsonl            # 每 run 一行，should-run 的唯一真相源
goals/<goal_id>/runs/<ts>[-quota-slot-spent].json + .md   # 双写
goals/<goal_id>/rollout-event-log.jsonl     # 审计
goals/<goal_id>/task-leases/                # 锁
```

真实记账行：

```json
{"generated_at":"2026-08-26T16:24:28+08:00","goal_id":"sls-bq-job-goal","classification":"quota_slot_spent",
 "recommended_action":"[P2] …","json_path":"…-quota-slot-spent.json","markdown_path":"…md","agent_id":"claude-code-…"}
```

对应 json 内 `quota_event:{event_type:"quota_slot_spent",source:"heartbeat",todo_id:"…",slots:1,before:{state:"eligible",spent_slots:3,allowed_slots:1440},after:{spent_slots:4}}`。

### 1.6 簇 A 结论：保留 / 砍掉

**保留（真正的 20%）：**

1. 滚动窗口 slot 账本：`allowed = window_h*60/slot_min*compute`，`spent = Σ窗口内 spend 事件`，append-only jsonl。
2. 状态梯：`paused > blocked > user_gate > waiting_external > throttled > eligible`。
3. `should_run = health_ok ∧ state==eligible ∧ 有 open agent todo`。
4. 先干活、写回带 outcome 的 run，再 `spend`（幂等键 = turn_id/todo_id）。
5. 节奏表：disposition → `[i,2i,4i]` + 3 次无变化停 + identity 变化重置。
6. 从单一状态文件的 todo 推 `waiting_on`（open user todo → gate；open agent todo → run）。

**整块砍掉：** agent identity/lane/peer coordination、capability gate、workspace guard、stall/projection/boundary self-repair、lark inbox、reward memory、research frontier、dreaming、vision checkpoint、canary、Codex App rrule/automation_update/scheduler-ack/TS state store、heartbeat receipt/settlement identity、handoff readiness、operator inbox、projection cache、interaction_contract/protocol_action_packet/turn_envelope 三层包装、`.md` 双写、heartbeat_prequota。

**最小 should-run 输入（10 个）：** `goal_id, goal_status, compute, window_hours, slot_minutes, spend_events[{ts,slots}], waiting_on, open_agent_todo_count, latest_run{ts,outcome}, now`。

## 2. 簇 B：状态模型与 Todo

### 2.1 ACTIVE_GOAL_STATE.md 的结构

- **frontmatter**：`status/owner_mode/objective/updated_at/adapter_id`。机器只读 `status`、只写 `updated_at`（`control_plane/todos/active_state_editing.py:279`）。
- **给人看的 section**：Objective / Authority Sources / Operating Contract / Execution Profile / Non-Goals / Recent User Feedback / Progress Ledger。后两者仅被 refresh-state 各读前 5 行塞进 run 记录，从不写回。
- **机读 section**（按标题关键词识别，`control_plane/goals/active_state_metadata.py:4-39`）：`## User Todo`（role=user）、`## Agent Todo`（role=agent）、`## Completed Work Archive`（done 超 12 条时归档）、`## Next Action`（一条 bullet + `<!-- loopx:next-action … todo_id=… -->`）。
- **Todo 行语法**（`control_plane/todos/contract.py:19-23`）：

```
- [ ] [P0] 文本…                      # 正则 ^\s*[-*]\s+\[([ xX-])\]\s+(.+?)$
  继续行（缩进、非注释）拼进 text
  <!-- loopx:todo k=v k=v … -->        # 缩进 2 空格，value 用 urllib.quote 编码（:811）
```

  - 复选框：空=open、`x`=done、`-`=deferred；**blocked 没有复选框形态，只存在于 `status=`**。`[P0..P4]` 只是文本前缀。
  - 注释字段全集 `_TODO_METADATA_FIELD_SCHEMA`（`contract.py:897-1183`）**约 50 个**：核心 15 个（`todo_id,status,task_class,action_kind,claimed_by,unblocks_todo_id,successor_todo_ids,completion_continuation,resume_when,no_followup,note,evidence,reason,completed_at,updated_at`）+ 多 agent 类 + 决策类 + 监控类 + 校验类。
  - `todo_id = "todo_" + sha1(role|section|index|text)[:12]`（`contract.py:756`），无注释时读取期临时合成。

### 2.2 Todo 状态机

- `status ∈ {open, done, blocked, deferred}`，终态 = done/deferred。
- 转移：`open↔blocked`（`todo update --status`）；`→deferred` 必须带 `resume_when`（`todo_done:<id> | pr_merged:#n | capacity_available:<cap>`），满足即 `resume_ready`；agent todo **只能经 `complete` 到 done**。
- `complete`（`todos.py:1647-2027`）：锁外先跑 `validation_command` → 加锁 → 算 `completion_continuation ∈ {no_followup, successor, active_goal}`（**Python 经 effect_runtime 调 TypeScript 算这 3 个枚举值**，`completion_state.ts:123`）→ 处理 `--next-agent-todo`（新建后继，继承 `[Pn]`，回链 `successor_todo_ids`）→ 若 Next Action 绑定的是该 todo，用下一个 open agent todo 重写 Next Action。
- `claimed_by`：agent todo 的执行归属。**单 agent goal 下不做任何归属校验**（`mutation_authority.py:205`）——即这套 claim/lease 对单人场景是空转。

### 2.3 "下一个可执行 todo"的选择算法

```
items = parse section(role)                               # 文件顺序 index=1..n
open  = [t for t in items if status not in {done,deferred}]
open.sort(key=(P_rank([Pn]前缀) or 50, index))            # projection.py:75-101
exec  = [t for t in open if status=="open"                 # blocked 出局
             and (not t.resume_when or t.resume_ready)
             and task_class(t)=="advancement_task"]        # monitor/user_gate/blocker 出局
if agent_id: exec = [t for t in exec if agent_id∉excluded_agents
                     and (not claimed_by or claimed_by==agent_id)]
next = exec[0]                                             # todo_summary.py:1142,1311
```

### 2.4 `refresh-state` 做什么（`state_refresh.py:853-1489`）

读 registry → goal → state 文件；可选在锁内重写 `## Next Action` + `updated_at`（写前比对全文防并发）；推导 recommended_action；组 run record `{generated_at, goal_id, classification, recommended_action, health_check, state{sha256_16,…}, agent_id, …}`；写 `runs/<ts>.json + .md`，追加 `runs/index.jsonl`；再同步全局 registry。**不改任何 todo 行**。
`--classification` 是自由字符串，只是 run 记录的标签（默认 `state_refreshed`），history/quota 用它过滤。

### 2.5 registry.json 真正必要的字段

`inspect_registry` 只硬校验：根 `goals[]`；每 goal `id, repo, domain, state_file, adapter.kind`（`registry.py:386-418`）。全局 `registry.global.json` 是各项目 goal 的净化副本，项目 registry 才是真源。

### 2.6 并发与原子写

- `exclusive_file_lock`（`file_lock.py:355-414`）：同目录 `<file>.lock`，`fcntl.flock(LOCK_EX|LOCK_NB)` 轮询（5s/50ms），holder JSON 写进锁文件。
- 所有 todo 变更都在锁内 read→改行→write。
- **Markdown 写入非原子**：直接 `Path.write_text` 原地覆盖（`todos.py:1165,1614,1999`），只靠锁保护；只有 JSON registry 才是 tmp+fsync+`os.replace`。
- `event_sourced_state`（events.jsonl）只在文件存在时作为替代源，样本 goal 没有 → 非核心。

### 2.7 簇 B 结论：保留 / 砍掉

**一个 `zloop.json`（真源）+ 可选 `STATE.md` 投影（只渲染不解析）** 即可覆盖 90% 场景。loopx 的痛点正是把 Markdown 当数据库：注释里塞 50 个 urlencode 字段、每次读都重新合成 id、写不原子。

- 最小 Todo（8 字段）：`id, text, priority(0-2), status(open|blocked|deferred|done), blocked_by[], note, updated_at, done_at`。
- 最小 state：`{version, goal:{id,objective}, updated_at, next_id, todos[], ticks[]}`（ticks 保留最近 N 条 `{at, kind, summary}`，即 runs/index 的精简）。
- 保留机制：4 态 + 终态判定；`(priority, index)` 排序 + blocked 过滤；`next_id` 指针 + done 后自动指向下一个；`done --next "…"` 一步生成后继；done 超阈值归档；sibling `.lock` + `tmp→os.replace` 原子写。
- 整块砍掉：user 角色 todo 及 `bound_agent/blocks_agent/global_gate/excluded_agents/claimed_by`、mutation_authority、handoff gate、task lease、decision_scope、capability_*、explore_result_node_refs、replan_obligation、vision checkpoint、settlement identity、全局 registry 同步、rollout-event-log、event_sourced_state、TS effect_runtime、lark 扩展、continuous_monitor 全套。

## 3. 簇 D：交互契约、引导事务与"难用"根因

### 3.0 核对数据

819 个 .py / 317,699 行。一级目录行数：capabilities 94,406（235 文件）> control_plane 89,748（276）> 根目录 63,392（111）> extensions 27,310 > cli_commands 21,792（75）> presentation 7,201 > canary 7,032。control_plane 内：todos 13,380 / quota 12,733 / testing 12,213 / work_items 12,186 / agents 9,062。`schema_version` 出现 2,573 次（505 文件），**不同字面值约 497 种**。

### 3.1 interaction_contract 生成逻辑

输入 = `quota should-run` 拼好的 payload（`should_run_packet.py:1023-1370`，**≥44 个顶层键**）。`mode` 由一条 if 链决定（`interaction_contract.py:478-557`），**约 30 种**（bounded_delivery、user_gate、monitor_due、autonomous_replan、outcome_floor_recovery、capability_bridge_repair、control_plane_self_repair、health_blocked、quota_throttled、terminal_no_followup、peer_coordination_blocked……）。

```
build_interaction_contract(payload):                                    # :1411
  mode          = if_chain(effective_action, state, execution_obligation, flags)
  user_required = 有 open user todo / gate
  must_attempt  = execution_obligation.must_attempt_work and not user_required
  primary_action = first_of(agent_lane_next_action, monitor_due_items,
                            capability_gate.runnable, agent_todo_summary.first_executable, recommended_action)
  spend = f"loopx quota spend-slot --goal-id {g} --slots 1 --source heartbeat --execute --todo-id {t} --agent-id {a}"
  if mode in DELIVERY_MODES:
      next_cli = [f"loopx refresh-state --goal-id {g} --classification <validated_progress> --agent-id {a}", spend]
  elif mode in USER_MODES: next_cli = ["no quota spend for blocker-push/gate-notification"]
  elif monitor/replan/frontier modes: 各自专用 2-4 条命令
  return {mode, user_channel{action_required,notify}, agent_channel{must_attempt,delivery_allowed,primary_action},
          cli_channel{next_cli_actions, spend_allowed_now=False, spend_after_validation}}
```

正常交付路径下 `next_cli_actions` 恒为两条：refresh-state + spend-slot。

### 3.2 `start-goal --guided` 8 步中哪些真正必要

CLI 返回的是 `mode: dry_run_preview, writes_now: False`——**只描述事务，由模型手工回放**。

| 步 | 单人单机 | 判断 |
|---|---|---|
| inspect_connection | 可并入 connect | 只是"goal 存在吗" |
| connect_if_needed | **必要** | 创建状态文件 |
| select_agent_identity | 多 agent 税 | 多 lane / 线程绑定；且注册后必须**重跑一次** start-goal |
| plan_ranked_todos | **必要** | 唯一 model_checkpoint |
| write_ordered_todos | **必要** | 但模板强制 `--claimed-by/--task-class/--action-kind`，claimed_by 还要先注册 |
| refresh_state | 单机应自动 | 状态是单文件时写 todo 即写状态 |
| activate_host_loop | 多宿主税 | Codex App RRULE/heartbeat |
| quota_guard | 概念必要 | 但混入 slot/scheduler/capability |

另有 4 条条件步（bind_thread_identity、configure_fine_grained_turn_mode、qualify_selected_capability、scheduler_ack_when_needed）全是多宿主/安全边界。**真正主干只有 3+1 步：建状态 → 规划 → 写 todo → 问该不该跑。** task lease（1399 行，TTL 45min/24h）仅 `handoff_mode=hard_lease` 时生效，默认 legacy——单 agent 完全不需要。

### 3.3 "难用"的 8 条结构性根因（有证据）

1. **命令面爆炸**：argparse 实测 **113 个顶层子命令、307 个叶命令、2,553 个 flag 定义、893 个唯一 flag 名**（`cli.py:224`）；其中 16 个 `codex-cli-*` 宿主专用命令。
2. **"flag 并集"反模式**：`todo` 是单 parser + 9 个位置动作 + **75 个 flag**，帮助文本自述"下面的选项是所有 todo 命令的并集…不支持的组合在读状态前失败"（`cli_commands/todo.py:149-175`）；`configure-goal` 56、`quota` 52、`refresh-state` 44（其中 18 个 `--vision-*/--progress-*`）、`bootstrap` 37。
3. **每轮输出过大且自我重复**：`quota should-run` 输出预算本身就是 **20,000–30,000 字符 / 520 行 JSON**（`testing/cli_output_budget.py`），start-goal 40,000、bootstrap-command-pack 45,000、status 42,000。同一指令三处出现（`agent_channel.primary_action` / `protocol_action_packet` / `cli_channel.next_cli_actions`）；"是否必须执行"三处（`execution_obligation.must_attempt_work` / `heartbeat_recommendation.agent_must_attempt` / `agent_channel.must_attempt`）。产品内置 `duplication_measurement` 统计 objective 重复次数，并用 `legacy_fields_retained / removal_gate` 把冗余制度化。
4. **概念爆炸**：control_plane 内标识符计数——agent 622、monitor 406、quota 294、scope 292、projection 272、gate 247、lane 217、contract 199、receipt 179、handoff 160、frontier 156、settlement 127、capability 122、lease 115；一个 todo 有 22 个 `normalize_todo_*`；"谁在跑循环"用 4 个正交轴描述：host_surface × runtime_profile × scheduler_owner × execution_mode。
5. **schema_version 税**：497 种版本字面值，连内层 `delivery_workspace_causality_v0`、`loopx_packet_json_pointer_ref_v0` 都各带版本。
6. **状态分散 ≥9 处**：`.loopx/registry.json`、`<proj>/.codex/goals/<g>/ACTIVE_GOAL_STATE.md`、`~/.codex/loopx/registry.global.json`、`goals/<g>/runs/index.jsonl`、`goals/<g>/turns/`、`task-leases/`、status-projection-cache、chat、backups。todo 真相是被正则解析的 Markdown 复选框，上面压着 13,380 行的 todos 包。
7. **写回是 7 阶段事务 + 2 条命令 + 环境变量**：host_execute → typed_result → validation → durable_writeback → quota_spend → scheduler_apply → scheduler_ack；每轮模型要执行 refresh-state 再 spend-slot，并维护 `LOOPX_TURN` 幂等 id。
8. **启动仪式**：`start-goal` 无 `--guided` 直接拒绝，无 `--host-surface` 返回"选宿主"包，然后给一个只读的 8–12 步事务让模型自己回放（本项目实测 12 条 CLI 调用才开始干活）。
9. **写回顺序耦合且无补救**（本项目实测）：完成第一条 todo 时按直觉先 `todo complete` 再 `quota spend-slot --todo-id <该 todo>`，spend 的 preview 会重新跑 should-run，此时 selected_todo 已前移到下一条，于是报 `quota spend todo binding mismatch … complete or update the selected todo first`；去掉 `--todo-id` 又报 `quota spend requires --todo-id <下一条>`。**唯一合法顺序是 refresh-state → spend-slot → complete**，但 CLI 在 complete 时不提示、事后也没有 `--completed-todo-id` 之类的记账入口——一步走错，这一轮的配额账就永久丢失。zloop 的 `done` 把写回 + 记账 + 完成合成一条命令，从结构上消灭这个坑。

### 3.4 簇 D 结论：zloop 的 CLI 形态

主干 = goal → 有序 todo → 每轮 should-run → 执行一条 → 写回。**4 个子命令 + 1 个只读，每个 ≤3 个 flag**：

- `zloop init "<goal>"`：建单一 JSON 状态文件，不需要 agent/host/surface。
- `zloop plan`（stdin 或文件）：一次性写入有序 todo（替代 todo add/update/claim）。
- `zloop next [--json]`：即 should-run。只做三件事：挑第一条可执行 todo、检查停止条件（全部完成 / 连续失败 N 次 / 用户阻塞 / 配额用尽）、输出下方 JSON。
- `zloop done <id> --note "…" [--fail|--block]`：唯一写回命令，合并 refresh-state + spend-slot + complete；轮次计数自增，不要 lease、不要 LOOPX_TURN。
- `zloop status`：只读。

每轮模型读的 JSON ≤10 字段：

```json
{"goal":"…","round":7,"should_run":true,"reason":"open todo available",
 "todo":{"id":"t3","text":"…"},
 "remaining":4,"last_result":"t2 done: …","writeback":"zloop done t3 --note '<result>'"}
```

`should_run=false` 时 `reason` 携带 `all_done / blocked / exhausted / backoff`，`todo` 为 null。不设 mode 枚举、不设 channel 分层、不带任何 schema_version 之外的元数据。

## 4. 簇 C：Claude Code / Codex 宿主接入

### 4.0 结论先行

loopx 有**两代** Claude 接入。老一代 `claude_goal_mode/`（MCP server + `.claude/commands/loopx.md` + `.claude/loop.md` + 可选 PreToolUse hook）已无其它模块引用（全库 grep `loop.md` 仅命中该目录）。**本机实际安装的是新一代纯 SKILL.md 门面**（`slash_command_install.py`）：Claude 侧无 MCP、无 hook、无 statusline。**宿主只做一件事——周期性把一段 prompt 喂给模型；所有判断都在 CLI 里。**

### 4.1 Claude Code 接入路径

```
1 用户 /loopx <goal>                         ~/.claude/skills/loopx/SKILL.md（21 行）
2 模型 → loopx start-goal --guided --slash-command-arguments="$ARGUMENTS" --host-surface claude-code
3 CLI 返回 ordered_steps                      bootstrap_command_pack.py:1474-1543
   inspect_connection → connect_if_needed → [select_agent_identity]
   → plan_ranked_todos(模型) → write_ordered_todos(`loopx todo add`) → refresh_state
4 activate_host_loop = `loopx heartbeat-prompt --thin --goal-id G --agent-id A --runtime-profile claude_code`
                                              host_loop_activation.py:821-846
5 模型读 task_body → 用户执行 `/loop <task_body>`（Claude Code 内置 /loop）
6 每 tick：按 task_body → quota should-run → next_cli_actions[0] → refresh-state → spend
7 should_run=false 且 terminal → 停
```

- **必需只有 `loopx/SKILL.md`**（模板 `slash_command_install.py:200-219`，首行 `<!-- loopx-managed-slash-command:v1 -->` 标记用于幂等覆盖、不误伤用户文件 `:390`）。
- 老一代锦上添花件：MCP `loopx_mcp.py`（should_run/claim_task/complete_task 全是 shell 出 CLI 子命令）；statusline `goal_status.py`；`.claude/loop.md` 4 步协议（首行 `<!-- loopx:armed {goal_id,agent_id} -->` 即"已武装"标记）。
- hook 仅 `--harden` 时装，挂 **PreToolUse, matcher "\*"**（`install.py:56-65`）。`goal_policy.py:149-198`：只读工具直接 allow；否则跑 `quota should-run`，false 或探测失败 → deny（fail-closed）；true 时 Edit/Write 限 write_scope，Bash 查毁灭性命令黑名单。

### 4.2 Codex 接入路径

- 装法：`~/.codex/skills/<name>/SKILL.md` + `agents/openai.yaml`（`allow_implicit_invocation: false`）；Codex 不支持自定义顶层 slash，只能 `$loopx` 或 /skills 显式调（`slash_command_install.py:961-984`）。`~/.codex/loopx/` 是两宿主**共享**的 runtime root。
- **codex-app**：模型用 `automation_update` 建心跳自动化，body = thin task_body，初始 3 分钟（`host_loop_activation.py:700-725`）。之后每轮 should-run 返回 `scheduler_hint.codex_app{apply_needed, recommended_rrule, ack_hint}`，间隔 = 初值×2ⁿ、上限 60 分（`scheduler_hint.py:486-498`）；改 RRULE 后跑 ack，不花 quota。guard 带 `--turn-instance-id "${LOOPX_TURN:?}"` 产心跳回执。
- **codex-cli**：`loopx codex-cli-bootstrap-message` 生成一段可粘贴 setup → connect → `heartbeat-prompt --runtime-profile codex_cli`（去掉 RRULE/LOOPX_TURN，加"连续 3 轮无变化则 `update_goal status=blocked`"，`task_body.py:450-492`）→ `/goal <task_body>`，由 Codex 原生 goal 循环续跑。
- **headless turn_driver**（`loopx turn run-once --host codex-cli --execute`）：

```
status = collect_status(); env = live_should_run_decision(status)          turn.py:116-135
plan   = build_loopx_turn_plan(env, host)                                   driver.py:299
  route = should_run && delivery_allowed && must_attempt ? READY|REPAIR|REPLAN
        : user.action_required ? USER_ACTION : quiet_noop ? WAIT : BLOCKED
  session = binding(goal,agent,todo) 匹配 ? resume : start_new
run_loopx_turn_once(plan):                                                  executor.py:1327
  lock journal[turn_key]; 已 committed → 直接回放
  result = run_codex_cli_host(req):                                         codex_cli.py:384
    codex exec --skip-git-repo-check --sandbox read-only -C proj
      --output-schema s.json --output-last-message out.json --json [resume sid] -
    stdin ← "Execute exactly one bounded LoopX Turn… return schema JSON"
    result_kind ∈ validated_progress|repair_required|replan_required|user_action_required|wait
  settlement: DURABLE_WRITEBACK(refresh-state) → QUOTA_SPEND(spend-slot) → [TERMINAL_CLOSEOUT]
  scheduler(): 再跑 should-run 取 scheduler_hint                              turn.py:808-845
decide_loop_disposition(receipt, env) → run_now|wait|user_action|repair|replan|terminal
```

`codex_cli_scheduler.py` 只是 launchd 一次性 tick：打印候选命令或 blocker，不跑 Codex；真正执行需要 4 层批准，headless 默认禁用。

### 4.3 `--thin` task_body 的"每轮协议"（7 条）

渲染 `task_body.py:639-700`，预算 1900 字符（`budget.py:12`）：

1. 每轮先 `LOOPX_TURN=<now>`（重试复用），跑 `loopx --format json quota should-run --goal-id G --agent-id A --runtime-profile …`。
2. **逐字执行 `interaction_contract.cli_channel.next_cli_actions[0]`**。bounded_delivery 时列表 = [`refresh-state --classification …`, `quota spend-slot --slots 1 --execute`]；user_gate 时 = "不花 quota"。
3. `should_run=false`：不干活不花 quota；`notify=NOTIFY` 才向用户输出；执行义务只看 `must_attempt_work`。
4. `should_run=true`：取最高优先未阻塞 todo 做一个有界片段 → 验证 → 写回 → **恰好 spend 一次**；无变化写 `--vision-unchanged-reason`，不 spend。
5. 完成 todo → 先建 successor；最终 → refresh → spend → no-follow-up；连续 2 轮无进展 → replan。
6. `scheduler_hint`：pause_or_delete → 停自动化；否则应用 RRULE/回退/ack，均不花 quota。
7. P0 阻塞可做安全 P1/P2；禁止私有材料、凭证、破坏性 git、未授权生产操作。

### 4.4 两宿主共享什么、差在哪

**共享**：同一份状态（registry + ACTIVE_GOAL_STATE.md + `~/.codex/loopx` runtime）、同一个 `quota should-run`（仅 `--runtime-profile claude_code|codex_app_heartbeat|codex_cli` 不同）、同一个 `build_heartbeat_prompt`（按 profile 选渲染器）、同一套 `next_cli_actions`。

**差异**：① 触发者——Claude `/loop`（模型自定步，用户在场）/ Codex App RRULE（宿主调度 + backoff + ack）/ Codex CLI 原生 `/goal`（宿主拥有 blocked 状态）/ headless `turn run-once`（loopx 自己拥有循环）；② 回执——codex_app 强制 `--turn-instance-id`，其他不要；③ 渲染器 thin vs visible_goal；④ 强制力——Claude 可加 PreToolUse hook 逐工具拦截，Codex 只有 prompt 纪律；⑤ Codex 用 `CODEX_THREAD_ID` 做 thread→agent 绑定，Claude 无。

### 4.5 agent 注册 / 身份 / thread binding 对单人单机是否必要

目的全是**多 agent 并行同一 goal**：lease 冲突、按 lane 记进度、blocker 推给别的 agent。`should-run` 缺 `--agent-id` → `automation_prompt_upgrade_required, should_run=false`（`goal_policy.py:35-39`）。**单人单机单 agent：不必要。** 砍掉只失去多宿主同跑同一 goal 的碰撞保护。建议固定隐含 id（如 `self`），删注册、gate、thread binding。

### 4.6 簇 C 结论：最小宿主接入层（4 个文件）

- `hosts/SKILL.md` 一份模板，装到 `~/.claude/skills/zloop/` 与 `~/.codex/skills/zloop/`（Codex 多一个 `agents/openai.yaml`），带 managed 标记。正文 5 行：`$ARGUMENTS` 原样传 `zloop start "<args>"`，输出照抄；Claude 接 `/loop zloop tick`；Codex App 建 3 分钟 automation、body = `zloop heartbeat`；Codex CLI `/goal <输出>`。
- `zloop heartbeat`：输出 ≤1900 字符 task_body（即 4.3 的协议压成 5 条）。
- `zloop tick`（= next）：合并 should-run + next_cli_actions，返回 `{should_run, reason, todo, writeback, next_interval_min}`；模型只需"跑 tick，做 todo，跑 writeback"。
- 可选 `hosts/claude_stop_hook.py`：挂 **Stop**（而非 PreToolUse）——调 `zloop tick`，should_run 为真则输出 `{"decision":"block","reason":"<下一轮 prompt>"}`，Claude Code 不靠 `/loop` 也能确定性续跑。

最小安装动作：**Claude Code** = 写 1 个 SKILL.md（不装 MCP/hook/statusline）；**Codex** = 写 SKILL.md + openai.yaml 到 `~/.codex/skills/zloop/`（不碰 prompts/hooks/config.toml）。

---

## 5. 跨簇总结：zloop 保留 / 砍掉总表

### 5.1 保留（loopx 里真正有价值的 ≈ 6 个想法）

| # | 机制 | 来源 | zloop 落法 |
|---|---|---|---|
| 1 | **有序 Todo 队列 + 优先级 + 阻塞依赖** | 簇 B | `todos[]` 按 `(priority, index)` 排，`blocked_by[]` 未满足则跳过 |
| 2 | **should-run 状态梯** `paused > blocked > user_gate > throttled > eligible` | 簇 A | `zloop next` 一个函数、≤10 输入字段 |
| 3 | **滚动窗口配额** `allowed = window_h*60/slot_min*compute`，append-only 记账 | 簇 A | `ticks[]` 里 `{at, todo, outcome}`，窗口内计数 |
| 4 | **先干活再记账**（spend after validated writeback）+ 幂等键 | 簇 A/D | `zloop done <id>` 一条命令合并写回 + 记账，幂等 = `(id, round)` |
| 5 | **backoff 节奏表** `[i,2i,4i]` + 3 次无变化停 + 状态变化重置 | 簇 A | `next_interval_min` 字段直接给 `/loop` / RRULE |
| 6 | **同一份状态 + 同一个 tick + 同一段 prompt，服务多宿主** | 簇 C | `zloop heartbeat` 输出 task_body，SKILL.md 模板复用 |

另外两条工程习惯值得保留：sibling `.lock` + `tmp→os.replace` 原子写；managed 标记（`<!-- zloop-managed -->`）让安装幂等且不误伤用户文件。

### 5.2 砍掉（占 loopx ≥ 90% 代码）

- **多 agent**：agent 注册 / lane / lease / thread binding / peer coordination / handoff / supervisor / multi_agent/ / botmux
- **多宿主矩阵**：host_surface × runtime_profile × scheduler_owner × execution_mode 四轴；16 个 `codex-cli-*` 命令；turn_driver 7 阶段 settlement + journal + TS reduce
- **能力/插件层**：capabilities/（94k 行：issue_fix、auto_research、content_ops、benchmark_toolkit、change_quality、connector_registry…）
- **扩展**：extensions/lark（27k 行）、openviking、dashboard/chat/web/desktop、canary、dreaming、reward memory、vision checkpoint、research frontier
- **契约包装**：interaction_contract 30 种 mode、protocol_action_packet、turn_envelope、497 种 schema_version、duplication_measurement
- **状态分散**：全局 registry 同步、runs 双写 .json+.md、rollout-event-log、event_sourced_state、projection cache、Markdown 注释里 50 个 urlencode 字段
- **仪式**：`--guided` 8–12 步只读事务、`--host-surface` 选择 gate、`LOOPX_TURN` 环境变量、`heartbeat_prequota`

### 5.3 zloop 的一句话定义

> **一个 JSON 文件 + 8 个子命令 + 1 个 SKILL.md 模板**：`init` 建目标、`plan` 写有序 todo、`next` 回答"现在该不该跑、跑哪条"、`done` 一次写回并记账、`edit` 改 todo、`status` 只读、`heartbeat` 吐给宿主的 prompt、`install` 装 skill。Claude Code 用 `/loop /zloop` 续跑，Codex 用同一段 prompt 建 automation。目标规模：≤ 1,000 行 Python、零依赖、每轮模型读的 JSON ≤ 10 个字段。

详细架构见 `docs/DESIGN.md`；实现结果与精简度对比见 `README.md`。
