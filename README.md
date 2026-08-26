# zloop

**让 Claude Code / Codex 围着一个目标持续干活的最小调度器。**
一个 JSON 文件、12 个子命令、零运行时依赖、单个 1.2 MB 的 Rust 二进制。

除了基本循环（init / plan / next / done / edit / status / heartbeat / install），它专门解决四件事（设计见 [docs/RUST-DESIGN.md](docs/RUST-DESIGN.md)）：

| 目标 | 命令 | 怎么实现 |
|---|---|---|
| **任务长时间运行** | `zloop run --host claude\|codex` | 无头驱动 `claude -p` / `codex exec`，一轮一条 todo，按 `next` 的 interval 睡眠，所有停机条件来自 `next`；journal 记 begin/end，`kill -9` 后重启从当前状态续 |
| **跨 Claude Code / Codex 切换的上下文** | `zloop context [--for codex]` | ≤4000 字符的交接包：目标 / 当前判断（最近 3 次执行）/ 下一条 / 待办 / 各宿主会话 / 怎么继续；两个宿主本来就读同一个状态文件 |
| **执行留档** | `zloop done … --evidence "…\|@file"` → `zloop log` | 每次 `done` 生成 `.zloop/log/<ts>-<todo>-<outcome>.md`（目标、todo、结果、宿主、会话、resume 命令、证据） |
| **进入对应的 resume 会话** | `zloop sessions` | 每个 tick 自动记录 `CLAUDE_CODE_SESSION_ID` / `CODEX_THREAD_ID`；打印可直接执行的 `claude --resume <id>` / `codex resume <id>`，并检查 transcript 是否存在 |

实测：`next` 一次调用 **12 ms**（同逻辑的 Python 原型 53 ms）；`cargo test` 38 个用例；runner 用 `claude -p` 无人值守跑 2 轮约 90 秒，第 2 轮自动 `--resume` 第 1 轮会话。

zloop 是对 [loopx](https://github.com/huangruiteng/loopx) 里"Claude Code / Codex 核心调度"那 20% 的重写：保留"状态 → 该不该跑 → 跑一条 → 写回 → 决定下一 tick"这条主干，砍掉多 agent、能力插件、仪表盘、飞书、30 种交互模式和 32 万行代码。loopx 的设计思路（长时运行如何不中断、上下文怎么管、多 agent / 多宿主怎么协同）见 [docs/loopx-principles.md](docs/loopx-principles.md)；保留了什么、砍了什么见 [docs/loopx-scheduling-notes.md](docs/loopx-scheduling-notes.md)；zloop 架构见 [docs/DESIGN.md](docs/DESIGN.md)。

## 安装

```bash
git clone <this repo> zloop && cd zloop
cargo build --release                # Rust ≥ 1.75；产物 target/release/zloop（约 1.2 MB）
install -m755 target/release/zloop ~/.local/bin/zloop
zloop install --claude               # 写 ~/.claude/skills/zloop/SKILL.md
zloop install --codex                # 写 ~/.codex/skills/zloop/{SKILL.md,agents/openai.yaml}
```

可选：`zloop install --claude-stop-hook` 往 `~/.claude/settings.json` 加一个 Stop hook——有可执行 todo 时阻止 Claude 停下并把协议塞回去，不需要 `/loop`；所有 todo 做完、等人或连续失败 3 次时自动放行。要停用就删掉 settings.json 里 `hooks.Stop` 中 `zloop hook-stop` 那一条。

`install` 只写带 `<!-- zloop-managed:v1 -->` 标记的文件，重复执行幂等，遇到不是它写的同名文件会拒绝覆盖。默认**不**安装任何 hook。

## 60 秒上手

```bash
cd my-project
zloop init "把 demo 服务的启动时间降到 1 秒以内"

printf '[P0] 测量当前启动耗时并记录基线
[P0] 找出最慢的 3 个初始化步骤
[P1] 对最慢步骤做懒加载
[P2] 写优化说明\n' | zloop plan

zloop next --json
# {"goal": "...", "round": 0, "should_run": true, "reason": "ready",
#  "todo": {"id": "t1", "text": "测量当前启动耗时并记录基线", "priority": 0},
#  "remaining": 4, "last": null,
#  "writeback": "zloop done t1 --note '<一句话结果>'", "interval_min": 3}

zloop done t1 --note "基线 3.2s，脚本 bench.sh"
zloop done t2 --outcome progress --note "已定位 2 个，第 3 个待查"
zloop done t2 --block "第 3 个步骤涉及付费 SDK，是否允许替换？"   # t2 等人，next 会自动去跑 t3
zloop status
zloop status --md > .zloop/STATE.md      # 只读投影，给人看的
```

## 每轮协议（模型看的就是这 5 条）

```
1. 运行 `zloop next --json`。should_run=false 时，按 reason 简短告知用户后停止本轮。
2. should_run=true 时，只做 todo 里这一条：做出可验证的产物，能跑的就跑一下验证。
3. 完成 → `zloop done <id> --note "…"`；有进展没做完 → --outcome progress；失败 → --outcome fail；
   需要用户决定 → --block "<问题>"；发现新任务 → --next "<任务>"。
4. 不要改 .zloop/ 以外的调度状态；不碰凭证、不做破坏性 git、不做生产操作。
5. 每轮结束用两三句话告诉用户：做了什么、验证了什么、下一条是什么。
```

`zloop heartbeat --host claude|codex-app|codex-cli` 会把这 5 条连同目标和目录打印出来（约 1,100 字符），最后一句因宿主而异：怎么续跑。

## 在 Claude Code 里用

```
/zloop 把 demo 服务的启动时间降到 1 秒以内     ← 初始化 + 规划 todo + 跑第一轮
/zloop                                        ← 再跑一轮
/loop /zloop                                  ← 让 Claude Code 自己按 interval_min 续跑
```

可选（实验性）：`zloop install --claude-stop-hook` 往 `~/.claude/settings.json` 加一个 Stop hook。有可执行 todo 时它会阻止 Claude 停下并把协议塞回去，不需要 `/loop`；所有 todo 做完、连续失败 3 次或等人时它自动放行。

## 在 Codex 里用

- **Codex App**：让模型用 `automation_update` 建一条 automation，body 就是 `zloop heartbeat --host codex-app` 的输出，初始间隔 3 分钟；每轮 `interval_min` 为 `null` 时暂停。
- **Codex CLI**：`/goal <zloop heartbeat --host codex-cli 的输出>`，交给原生 goal 循环。

两种宿主读的是同一个 `.zloop/state.json`、跑的是同一个 `zloop next`。

## 命令

| 命令 | 作用 | flags |
|---|---|---|
| `zloop init "<goal>"` | 建 `.zloop/state.json` | `--force` |
| `zloop plan` | 写有序 todo：stdin / `--file` / `--add` / `--from-loopx` | `--add LINE`（可重复）, `--file`, `--replace`, `--from-loopx PATH` |
| `zloop next` | 该不该跑、跑哪条；空闲时记一笔 noop | `--json`, `--peek` |
| `zloop done <id>` | **唯一写回**：记 tick、改 todo 状态、可插后继 | `--note`, `--outcome progress\|fail`, `--block Q`, `--next LINE` |
| `zloop edit <id>` | 改文本 / 状态 / 优先级 / 依赖 | `--text`, `--status`, `--priority`, `--blocked-by t1,t2\|user\|''` |
| `zloop status` | 只读总览 | `--json`（整份状态）, `--md`（Markdown 投影） |
| `zloop heartbeat` | 打印每轮协议 | `--host claude\|codex-app\|codex-cli` |
| `zloop install` | 装 skill | `--claude`, `--codex`, `--claude-stop-hook` |
| `zloop sessions` | 出现过的宿主会话 + resume 命令 | `--host`, `--json` |
| `zloop context` | 有界交接包（换宿主 / 新会话先读它） | `--budget`, `--for claude\|codex\|cli` |
| `zloop log` | 列出 / 查看执行留档 | `--todo`, `--last`, `--show` |
| `zloop run` | 无头 runner | `--host claude\|codex`, `--max-rounds`, `--fast`, `--allow-all`, `--no-resume` |

所有命令接受全局 `--dir`；默认从当前目录向上找最近的 `.zloop/`。

### 无头长时运行

```bash
zloop run --host claude                 # 前台循环：next → claude -p（自动 --resume 上一轮会话）→ 校验写回 → 睡 interval_min
zloop run --host codex --max-rounds 5   # 换 Codex 跑 5 轮；两者共享同一状态，可以交替
zloop run --host claude --fast          # interval 按秒算，演示用
```

默认只放行 `Bash(zloop:*)` + 读写编辑工具（Claude）/ `--sandbox workspace-write`（Codex）；`--allow-all` 才跳过权限。模型一轮结束没有写回，runner 记一笔 `fail`，连续 3 次自动停。日志在 `.zloop/runner/journal.jsonl`。

### 切宿主 / 回看会话

```bash
zloop context --for codex     # 在 Codex 里第一步：读交接包
zloop sessions                # claude 36346c2a-… ticks 2 … ✓ transcript
                              #         claude --resume 36346c2a-…
zloop log --todo t2           # 这条 todo 的每次执行留档
```

## `next` 怎么决定

```
paused/done  >  all_done  >  user_gate / blocked  >  fail_streak  >  throttled  >  ready
```

- 有可执行 todo（open 且 `blocked_by` 全部完成）→ `ready`，选 `(priority, 写入顺序)` 最靠前的一条，`interval_min = 3`。
- 全部 blocked 且有人在等 → `user_gate`；纯依赖未满足 → `blocked`。退避 10 → 30 分钟，连续 3 次 noop 后 `interval_min = null`（停下等人）。
- 最近连续 3 次 `fail` → `fail_streak`，停；`zloop edit` 一下（人介入）即可重置。
- 24 小时窗口内已记账 60 次 → `throttled`，告诉你几分钟后窗口释放。

策略在 `state.json` 的 `policy` 里，直接改：`window_hours / max_runs / max_fail_streak / max_noop_streak / intervals_min`。

## 状态文件

```jsonc
{
  "version": 1,
  "goal":   { "id": "my-project", "text": "…", "status": "active", "created_at": "…" },
  "policy": { "window_hours": 24, "max_runs": 60, "max_fail_streak": 3, "max_noop_streak": 3, "intervals_min": [3, 10, 30] },
  "todos":  [ { "id": "t1", "text": "…", "priority": 0, "status": "open", "blocked_by": [], "note": "", "updated_at": "…", "done_at": null } ],
  "ticks":  [ { "at": "…", "round": 1, "todo": "t1", "outcome": "done", "note": "…" } ],
  "next_id": 2,
  "updated_at": "…"
}
```

写入是 `tmp → fsync → os.replace` 原子替换，并发靠同目录 `state.json.lock`（`fcntl.flock`）。JSON 是唯一真源；`STATE.md` 只渲染、不回读。

## 从 loopx 迁移

```bash
cd my-project
zloop init "$(grep '^objective:' .codex/goals/<goal>/ACTIVE_GOAL_STATE.md | cut -d'"' -f2)"
zloop plan --from-loopx .codex/goals/<goal>/ACTIVE_GOAL_STATE.md
```

只导入未勾选的 `- [ ] [Pn] …` 行（User Todo 与 Agent Todo 两节都算），剥掉 `<!-- loopx:todo … -->` 注释，`[P0]/[P1]/[P2]` 前缀原样保留；已完成 `[x]` 和延后 `[-]` 的不导入。loopx 的 `claimed_by`、`task_class`、`action_kind`、lease、successor 链等元数据没有对应物，直接丢弃。

## 精简度对比

| 维度 | loopx 0.5.2 | zloop 0.2 |
|---|---|---|
| 源码文件 / 行数 | 819 / 317,699（Python） | 10 / 2,190（Rust） |
| 顶层子命令 | 113（叶命令 307） | 12（+1 内部 `hook-stop`） |
| 单命令最多 flag | 75（`todo`） | 5（`done`，含 `--evidence`） |
| `next` 一次调用 | 20–30 KB JSON，数百 ms | 9 个字段，12 ms |
| 状态存放处 | ≥ 9 处（两级 registry、Markdown 状态、runs/、turns/、leases/…） | 1 个 JSON（+ 可读的 log/*.md） |
| Todo 元数据字段 | ≈ 50（URL 编码塞进 Markdown 注释） | 8 |
| 每轮写回 | `refresh-state` + `spend-slot` + `todo complete`，顺序错一步丢账 | `zloop done` 一条 |
| 开始干活前 | 8–12 步事务、12 条 CLI 调用、必须注册 agent 身份 | `init` + `plan` |
| 宿主 prompt | ≈ 1,900 字符 / 7 条规则 / 含 `LOOPX_TURN` 环境变量 | ≈ 1,200 字符 / 5 条规则 |
| 无头运行 | `turn run-once`（仅 codex-cli，7 阶段结算） | `run --host claude\|codex`，journal 两行 |
| 会话回看 | `turn-sessions/<sha>.json`（仅 headless codex） | 每 tick 记 host+session，`sessions` 直接给 resume 命令 |
| Claude Code 安装物 | 11 个 skill | 1 个 skill（+ 可选 Stop hook） |
| 运行时依赖 | 0 | 0（静态单二进制） |

## 明确不做

多 agent 协作、能力路由、仪表盘、聊天、飞书、事件溯源、全局跨项目 registry、PreToolUse 强制拦截。两个 agent 同时跑同一个 `.zloop/` 不会写坏文件，但会互相覆盖进度——这是有意为之。

## 开发

```bash
cargo test                       # 38 tests（tick / todo / state / cli）
cargo build --release && install -m755 target/release/zloop ~/.local/bin/zloop
```

目录：`src/` 实现（10 个模块）· `tests/` 集成测试 · `docs/` 设计与 loopx 研究笔记。v0.1 曾有一个 921 行的 Python 原型，v0.2 用 Rust 重写后已移除（其设计见 `docs/DESIGN.md`，状态文件格式兼容）。

MIT License.
