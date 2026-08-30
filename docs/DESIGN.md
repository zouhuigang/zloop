# zloop 设计文档（v0.1 Python 原型，历史记录）

> **状态：** 本文描述的 Python 实现已于 2026-08-27 被 Rust 版 v0.2 取代并从仓库移除。当前实现的设计见 `RUST-DESIGN.md`；本文保留的价值是数据模型、`next` 状态梯和 CLI 形态的推导过程，这些在 Rust 版中原样沿用。
>
> 一个 JSON 文件 + 8 个子命令 + 1 个 SKILL.md 模板，替代 loopx 里与 Claude Code / Codex 调度直接相关的那 20%。
> 依据：`docs/loopx-scheduling-notes.md`（对 loopx 0.5.2 的源码提炼）。
> 版本：v0 设计，2026-08-26。

## 1. 一句话

**zloop 让一个 agent（Claude Code 或 Codex）围着一个目标持续干活：每轮问一次"该不该跑、跑哪条"，干完一条写回一次，直到全部完成、被人阻塞或连续失败。** 它不管多 agent、不管能力插件、不管仪表盘。

## 2. 目标与非目标

### 目标

| # | 目标 | 可验证标准 |
|---|---|---|
| G1 | 单文件状态 | 一个项目只有 `.zloop/state.json`；没有全局 registry、没有 runs 目录、没有 Markdown 数据库 |
| G2 | 最小 CLI | 8 个子命令，每个 ≤ 4 个 flag；`zloop -h` 一屏看完 |
| G3 | 每轮输入极小 | 模型每轮读的 JSON ≤ 10 个顶层字段；heartbeat prompt ≤ 1200 字符 |
| G4 | 一条命令写回 | `zloop done` 一次完成"记录结果 + 记账 + 推进指针 + 可选生成后继"，不存在顺序坑 |
| G5 | 双宿主同一入口 | Claude Code（`/zloop` + `/loop`）与 Codex（automation / `/goal`）共用同一 `next` 与同一 prompt |
| G6 | 零依赖、小体积 | Python 3.11+ 标准库；核心 ≤ 1,000 行；测试 ≤ 600 行 |
| G7 | 从 loopx 迁移 | 能把 `ACTIVE_GOAL_STATE.md` 里的 todo 一次导入（P2） |

### 非目标（明确不做）

- 多 agent 协作：无 agent 注册、lane、lease、thread binding、peer coordination、handoff。
- 能力/插件层：无 issue-fix、auto-research、content-ops 等 capability route。
- 多宿主矩阵：无 host_surface × runtime_profile × scheduler_owner × execution_mode 四轴；Claude Code / Codex 之外的宿主靠同一段 prompt 自行接入。
- 产品化外围：无 dashboard、chat、lark、canary、dreaming、reward memory、event sourcing、rollout log。
- 治理契约：无 interaction_contract 30 种 mode、无 schema_version 泛滥、无 duplication_measurement。
- 强制拦截：不默认安装 PreToolUse hook；纪律靠 prompt。Stop hook 是可选实验项（§7.3）。

## 3. 总体架构

```
                    ┌──────────────────────────────┐
  Claude Code ──/zloop──▶│                              │
  (/loop 续跑)       │        zloop CLI (Python)     │──▶ .zloop/state.json  (唯一真源)
  Codex App ──automation─▶│  init · plan · next · done   │──▶ .zloop/STATE.md   (只读投影, 可选)
  Codex CLI ──/goal──▶│  status · heartbeat · install │──▶ .zloop/state.json.lock
                    └──────────────────────────────┘
```

三个原则：

1. **CLI 是唯一决策者**，宿主只负责"周期性把 prompt 喂给模型"（这一点直接继承 loopx，是它最对的设计）。
2. **JSON 是唯一真源**，Markdown 只渲染不解析（修正 loopx 把 Markdown 注释当数据库的错误）。
3. **每轮协议只有三步**：`zloop next --json` → 做 todo → `zloop done <id> …`。

## 4. 数据模型

### 4.1 `.zloop/state.json`

```jsonc
{
  "version": 1,
  "goal": {
    "id": "zloop",                          // 目录名派生，公共安全
    "text": "重写一个精简版的 zloop …",
    "status": "active",                     // active | paused | done
    "created_at": "2026-08-26T23:00:00+08:00"
  },
  "policy": {
    "window_hours": 24,                     // 滚动窗口
    "max_runs": 60,                         // 窗口内最多记账 run 数（loopx 默认 1440，太宽等于没有）
    "max_fail_streak": 3,                   // 连续失败即停
    "max_noop_streak": 3,                   // 连续无事可做即停
    "intervals_min": [3, 10, 30]            // 有活/退避一级/退避二级，单位分钟
  },
  "todos": [
    {
      "id": "t1",
      "text": "研读 loopx 核心调度链路，产出 notes",
      "priority": 0,                        // 0=P0 1=P1 2=P2
      "status": "done",                     // open | blocked | deferred | done
      "blocked_by": [],                     // todo id 列表；特殊值 "user" 表示等人
      "note": "docs/loopx-scheduling-notes.md 485 行",
      "updated_at": "…",
      "done_at": "…"
    }
  ],
  "ticks": [                                // 追加日志 = loopx runs/index.jsonl 的精简
    { "at": "…", "round": 1, "todo": "t1", "outcome": "done", "note": "…" }
  ],
  "next_id": 2                              // 下一个 todo 序号
}
```

字段总数：goal 4 + policy 5 + todo 8 + tick 5。loopx 的 todo 注释里有约 50 个字段，这里 8 个。

### 4.2 Todo 状态机

```
        plan/add           done --block "reason"
  (new) ───────▶ open ────────────────────────▶ blocked ──(人工 zloop edit / done --unblock)──▶ open
                  │  ▲
                  │  └── blocked_by 全部 done 后自动可执行（不改 status，只影响 executable）
                  │
                  ├── done <id>                    ──▶ done   （终态）
                  ├── done <id> --outcome progress ──▶ open   （记一笔 progress tick，指针不动）
                  ├── done <id> --outcome fail     ──▶ open   （记一笔 fail tick，fail_streak+1）
                  └── edit <id> --status deferred  ──▶ deferred（终态，不参与调度）
```

- **可执行**（executable）= `status == open` ∧ `blocked_by` 中每个 id 的 todo 都是 done ∧ `"user" ∉ blocked_by`。
- **排序** = `(priority, 写入顺序)`。同优先级按 `plan` 写入顺序，与 loopx 一致。
- **后继**：`done <id> --next "text"` 在该 todo 之后插入同优先级新 todo（对应 loopx 的 `--next-agent-todo`，但不需要 successor_todo_ids / unblocks_todo_id 双向回链——顺序就是链）。

### 4.3 tick.outcome

| outcome | 含义 | 计入 max_runs | 影响 |
|---|---|---|---|
| `done` | 完成一条 | 是 | round+1，fail_streak=0，noop_streak=0 |
| `progress` | 有进展未完成 | 是 | round+1，fail_streak=0 |
| `fail` | 尝试失败 | 是 | fail_streak+1 |
| `block` | 需要人 | 否 | todo→blocked(user)，noop_streak 不变 |
| `noop` | next 判定不该跑 | 否 | noop_streak+1（由 `next` 自动记录；`--peek` 不记） |
| `edit` | 人工改了 todo | 否 | 打断 fail_streak / noop_streak（人介入过了） |

## 5. `zloop next`：should-run 算法

loopx 的 11 种 decision、30 种 mode、44 个 payload 键，压成一个纯函数：

```python
def decide(state, now) -> Decision:
    g, p = state.goal, state.policy
    if g.status != "active":
        return Decision(False, g.status, interval=None)                 # paused / done → 停

    todos = sorted(open_todos(state), key=lambda t: (t.priority, t.index))
    execu = [t for t in todos if executable(t, state)]
    if not todos:
        return Decision(False, "all_done", interval=None)
    if not execu:
        reason = "user_gate" if any("user" in t.blocked_by for t in todos) else "blocked"
        return Decision(False, reason, interval=backoff(state, level=1))

    if fail_streak(state) >= p.max_fail_streak:
        return Decision(False, "fail_streak", interval=None)            # 连续失败 → 停，等人
    if noop_streak(state) >= p.max_noop_streak:
        return Decision(False, "noop_streak", interval=None)

    spent = count_ticks(state, since=now - p.window_hours, outcomes={"done","progress","fail"})
    if spent >= p.max_runs:
        return Decision(False, "throttled", interval=minutes_until_window_frees(state, now))

    return Decision(True, "ready", todo=execu[0], interval=p.intervals_min[0])
```

**状态梯**（与 loopx 一致，只是删掉 blocked_health / focus_wait / outcome_floor 等旁路）：

`paused/done > all_done > user_gate/blocked > fail_streak/noop_streak > throttled > ready`

**退避**：`backoff(level)` 取 `intervals_min[min(level + noop_streak, 2)]`，即 `[3, 10, 30]` 三档；任何 `done/progress` tick 把 noop_streak 清零回到 3 分钟。这是 loopx `[i, 2i, 4i]` + "3 次无变化停" 的直译。

### 5.1 `next --json` 输出（≤ 10 字段）

```json
{
  "goal": "重写一个精简版的 zloop …",
  "round": 2,
  "should_run": true,
  "reason": "ready",
  "todo": { "id": "t2", "text": "设计 zloop 精简架构并写 docs/DESIGN.md", "priority": 0 },
  "remaining": 4,
  "last": "t1 done: docs/loopx-scheduling-notes.md 485 行",
  "writeback": "zloop done t2 --note '<一句话结果>'",
  "interval_min": 3
}
```

`should_run=false` 时 `todo` 为 `null`，`reason ∈ {paused, done, all_done, user_gate, blocked, fail_streak, noop_streak, throttled}`，`interval_min` 为 `null` 表示"停，等人"。`next` 在 `should_run=false` 时自动追加一条 `noop` tick（loopx 需要模型手动 `--vision-unchanged-reason`）。

## 6. CLI 表面

| 命令 | 作用 | flags | 对应 loopx |
|---|---|---|---|
| `zloop init "<goal>"` | 建 `.zloop/state.json` | `--dir`, `--force` | `bootstrap` + `register-agent` + `configure-goal`（3 条、100+ flag） |
| `zloop plan` | 从 stdin/文件批量写有序 todo；`--add` 单条；`--from-loopx` 导入 | `--file`, `--add "[P1] text"`, `--replace`, `--from-loopx <ACTIVE_GOAL_STATE.md>` | `todo add` ×N（每条 7 个必填 flag） |
| `zloop next` | should-run + 选 todo | `--json`, `--peek` | `quota should-run` + `heartbeat-prompt` |
| `zloop done <id>` | 写回 + 记账 + 推进 | `--note`, `--outcome progress\|fail`, `--block "…"`, `--next "text"` | `refresh-state` + `quota spend-slot` + `todo complete`（3 条、顺序耦合） |
| `zloop edit <id>` | 改文本 / 状态 / 优先级 / 依赖 | `--text`, `--status`, `--priority`, `--blocked-by` | `todo update`（75 flag） |
| `zloop status` | 只读总览；`--md` 渲染 STATE.md 投影 | `--json`, `--md` | `status`（42KB 输出） |
| `zloop heartbeat` | 输出给宿主的 task_body | `--host claude\|codex-app\|codex-cli` | `heartbeat-prompt --thin --runtime-profile …` |
| `zloop install` | 装 SKILL.md 到宿主 | `--claude`, `--codex`, `--claude-stop-hook` | `slash-commands --install --surface …` |

`plan` 输入格式（每行一条，前缀可省略默认 P1）：

```
[P0] 研读 loopx 核心调度链路，产出 notes
[P0] 设计 zloop 精简架构并写 docs/DESIGN.md
[P1] 实现 zloop 核心 + pytest
[P1] 宿主接入 + 端到端冒烟
[P2] README + 迁移说明
```

`done` 是**唯一**写回命令。它在一次加锁事务里：追加 tick → 更新 todo 状态 → 可选插入后继 → 原子写回。不存在 loopx 那种"先 complete 后 spend 就丢账"的顺序坑（[loopx-scheduling-notes.md §3.3](loopx-scheduling-notes.md#33-难用的-9-条结构性根因有证据) 第 9 条）。

## 7. 宿主接入

### 7.1 每轮协议（heartbeat task_body，≤ 1200 字符）

```
你在为目标「{goal}」持续工作。每一轮：
1. 运行 `zloop next --json`。should_run=false 时，按 reason 简短告知用户后停止本轮，不要做别的。
2. should_run=true 时，只做 todo 里这一条：做出可验证的产物，能跑的就跑一下验证。
3. 完成 → `zloop done {id} --note "<一句话结果>"`；有进展没做完 → 加 --outcome progress；
   失败 → 加 --outcome fail；需要用户决定 → 加 --block "<问题>"；发现新任务 → 加 --next "<任务>"。
4. 不要改 .zloop/ 以外的状态；不碰凭证、不做破坏性 git、不做生产操作。
5. 每轮结束用两三句话告诉用户：做了什么、验证了什么、下一条是什么。
```

对比 loopx 的 7 条协议（[loopx-scheduling-notes.md §4.3](loopx-scheduling-notes.md#43---thin-task_body-的每轮协议7-条)）：删掉 LOOPX_TURN、逐字执行 next_cli_actions[0]、NOTIFY/DONT_NOTIFY 双通道、RRULE/ack、successor 先建后完成、P0 阻塞可做 P1/P2 等规则——它们要么是多宿主/多 agent 税，要么已被 `done` 一条命令吸收。

### 7.2 Claude Code

安装物：**只有** `~/.claude/skills/zloop/SKILL.md`（首行 `<!-- zloop-managed:v1 -->` 供幂等覆盖）。

```
用户 /zloop <goal 文本>
  → 模型执行 zloop init "<goal>"（已存在则报出当前目标，不覆盖）
  → 模型规划 2–5 条 todo，写成 [P0]/[P1]/[P2] 行，通过 stdin 交给 zloop plan
  → 模型执行 zloop next --json 并按 §7.1 做第一轮
  → 提示用户：输入 /loop /zloop 让它自动续跑

用户 /zloop（无参数）= 一轮 tick
  → zloop next --json → 做 → zloop done …
  → 输出末尾附 interval_min，供 /loop 动态节奏参考
```

SKILL.md 正文约 15 行，没有 host-surface 选择、没有 identity gate、没有 8 步事务。

### 7.3 可选：Claude Code Stop hook（实验）

`zloop install --claude-stop-hook` 往 `~/.claude/settings.json` 的 `hooks.Stop` 加一条 `zloop hook-stop`。该命令跑 `next`：`should_run=true` 输出 `{"decision":"block","reason":"<§7.1 协议 + 当前 todo>"}`，否则输出空（放行停止）。停止条件完全由 `next` 的状态梯保证（all_done / fail_streak / noop_streak / user_gate 都会放行），不会死循环。相比 loopx 的 PreToolUse "*" 拦截，Stop hook 只在"该不该继续"这一个点介入，不干预工具调用。

### 7.4 Codex

安装物：`~/.codex/skills/zloop/SKILL.md` + `agents/openai.yaml`（`allow_implicit_invocation: false`）。

- **Codex App**：模型用 `automation_update` 建 automation，body = `zloop heartbeat --host codex-app` 的输出，初始间隔 3 分钟；每轮 `next --json` 的 `interval_min` 给模型作为是否调整 RRULE 的依据（zloop 不做 ack、不做 scheduler-state 持久化——interval 本身就是从 ticks 推出来的，重算即可）。
- **Codex CLI**：`/goal <zloop heartbeat --host codex-cli 的输出>`，由 Codex 原生 goal 循环续跑；heartbeat 文本末尾加一句"连续 3 轮 should_run=false 则 `update_goal status=blocked`"。

两宿主差异只体现在 `heartbeat --host` 的最后一两句，其余完全相同。

## 8. 代码布局与体积预算

```
zloop/
  __init__.py        # 版本号
  state.py           # load/save（tmp+fsync+os.replace）、sibling .lock（fcntl.flock 轮询）、schema 校验   ~150 行
  todo.py            # 解析 [Pn] 行、排序、executable 判定、状态转移                                        ~120 行
  tick.py            # decide()、streak 计算、窗口计数、backoff                                             ~120 行
  prompt.py          # heartbeat task_body 模板（三宿主变体）+ STATE.md 渲染                                ~100 行
  hosts.py           # SKILL.md / openai.yaml 模板（内嵌字符串）与幂等安装、Stop hook 注入                  ~120 行
  cli.py             # argparse：8 个子命令 + hook-stop；所有输出 JSON/纯文本                               ~280 行
tests/
  test_tick.py       # 状态梯每个分支、窗口边界、streak、backoff
  test_todo.py       # 解析、排序、blocked_by、后继插入、状态机、loopx 导入
  test_state.py      # 原子写、锁竞争、损坏文件拒绝加载
  test_cli.py        # 端到端：init → plan → next → done → edit → status；install 幂等；hook-stop
docs/
  loopx-scheduling-notes.md
  DESIGN.md
```

预算：核心 ≤ 1,000 行（含注释），测试 ≤ 600 行。对比 loopx：819 文件 / 317,699 行 / 113 顶层子命令 / 2,553 flag。
（实现结果见 README「精简度对比」；宿主模板直接内嵌在 `hosts.py`，不再单独放 `hosts/` 目录，避免两处维护。）

## 9. 与 loopx 的对照表

| 维度 | loopx 0.5.2 | zloop v0 |
|---|---|---|
| 状态文件 | ≥ 9 处（registry ×2、ACTIVE_GOAL_STATE.md、runs/、turns/、leases/ …） | 1 个 `.zloop/state.json`（+ 可选只读 STATE.md） |
| Todo 元数据 | Markdown 注释内约 50 个 urlencode 字段 | JSON 8 字段 |
| 顶层子命令 | 113 | 8 |
| 单命令最大 flag 数 | 75（`todo`） | 4 |
| should-run 输出 | 20–30 KB / 44+ 顶层键 / 11 种 decision / 30 种 mode | ≤ 10 字段 |
| 每轮写回 | refresh-state + spend-slot（+ complete），顺序耦合 | `done` 一条 |
| 启动 | 8–12 步只读事务，模型回放，12 条 CLI | `init` + `plan` 两条 |
| agent 身份 | 必须注册，缺 `--agent-id` 即拒跑 | 无 |
| 宿主 prompt | 1,900 字符、7 条规则、含环境变量 | ≤ 1,200 字符、5 条规则 |
| 依赖 | 0（运行时） | 0 |
| 代码量 | 317,699 行 | ≤ 1,000 行 |

## 10. 实施顺序（对应 LoopX 里的后续 Todo）

1. **P1 实现核心**：`state.py → todo.py → tick.py → cli.py`，先让 `init/plan/next/done/status` 在 pytest 下全绿。
2. **P1 宿主接入**：`prompt.py + hosts.py`，`zloop install --claude` 装好后在本会话用 `/zloop` 跑一遍端到端冒烟（init → plan → next → done → status）。
3. **P2 文档发布**：README、从 loopx 迁移（`zloop plan --from-loopx <ACTIVE_GOAL_STATE.md>` 只导入未勾选的复选框行、剥掉 `<!-- loopx:todo -->` 注释）、精简度对比。

## 11. 已知取舍

- **没有多 agent 保护**：两个 agent 同时跑同一 `.zloop/` 会互相覆盖 tick——用文件锁保证写不损坏，但不保证语义。这是有意为之。
- **没有全局视图**：想看"所有项目的 zloop 状态"需要另写一个扫描 `~/work/*/.zloop/state.json` 的只读脚本（P2 之后再议）。
- **Stop hook 是实验**：Claude Code 的 Stop hook 行为随版本变化，默认不装。
- **max_runs 默认 60/天**：loopx 默认 1440 等于不限；60 意味着每 24 分钟最多一轮记账，防止 `/loop` 空转烧额度。可用 `edit` 改 policy（v0 先直接改 JSON）。
