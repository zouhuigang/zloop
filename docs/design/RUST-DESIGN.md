# zloop Rust 版设计（zloop-rs）

> 目标：把 Python 版 zloop（`zloop/`，921 行）改写为单二进制 Rust 程序，并在保持"一个 JSON 文件 + 极少命令"的前提下，补上四项能力：**① 任务长时间运行；② 在 Claude Code 与 Codex 之间切换时上下文不丢；③ 每次执行留下可读文档；④ 能直接进入对应的 `claude --resume` / `codex resume` 会话看细节。**
> 依据：`docs/loopx-principles.md`（loopx 的思路）、`docs/loopx-scheduling-notes.md`（loopx 的实现）、`docs/DESIGN.md`（Python 版设计）。凡 loopx 已验证的机制，尽量照搬；凡 loopx 为多 agent / 多宿主矩阵付出的复杂度，继续不要。
> 日期：2026-08-27。

## 1. 四个目标 → 验收标准

| # | 目标 | 借鉴 loopx 的什么 | 验收标准（可执行） |
|---|---|---|---|
| G1 | 任务可以长时间运行 | outer_controller 模式（`turn run-once`）：loopx 自己持循环，宿主只是执行器；spend-after-writeback；journal 幂等；fail/noop streak 停机 | `zloop run --host claude --fast --max-rounds 3` 在 demo 项目上无人值守跑完 3 轮；中途 `kill -9` 后重启，不产生重复 tick，从断点继续 |
| G2 | 上下文管理，可在 Claude Code / Codex 切换 | "真源在文件不在会话"；thin prompt + should-run 投影 + 冷路径按需拉；handoff packet（16 行 / 1800 字符预算，按段落逐级删） | 同一 `.zloop/state.json` 被两个宿主交替驱动（round 1 claude、round 2 codex）；`zloop context` 输出 ≤ 4000 字符的交接包，另一宿主读它即可续做，无需翻聊天记录 |
| G3 | 任务执行情况留文档 | runs/`<ts>.md` 双写、Progress Ledger、compact run index | 每次 `done` 生成 `.zloop/log/<ts>-<todo>-<outcome>.md`（含目标、todo、结果、证据、宿主、会话、resume 命令）；`zloop log` 列出；`STATE.md` 投影带会话链接 |
| G4 | 能进入对应的 Claude resume 会话 | loopx 的 `turn-sessions/<sha>.json` 记 codex session_id 供 `codex exec resume`；thread binding `(host_surface, thread_id)` | 每个 tick 记录 `host` + `session`；`zloop sessions` 打印可直接执行的 `claude --resume <id>` / `codex resume <id>`；id 对应的 transcript 文件真实存在 |

**已验证的技术前提（2026-08-27 本机实测）**：

- Claude Code 会话内执行的任何子进程都能读到环境变量 **`CLAUDE_CODE_SESSION_ID`**（本会话为 `11111111-…`），transcript 位于 `~/.claude/projects/<cwd 路径把 / 换成 ->/<session_id>.jsonl`；`claude --resume <id>` 交互续接，`claude -p --resume <id> "<prompt>"` 无头续接，`--fork-session` 可分叉。
- Codex 会话内有 `CODEX_THREAD_ID`（loopx `_host_thread.py:11-18` 用它做 thread binding）；`codex resume <SESSION_ID>` 交互续接，`codex exec resume <id> "<prompt>"` 无头续接，`codex exec --json --output-last-message <file>` 拿结构化结果；会话文件在 `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`。
- 无头驱动 Claude 需要 `claude -p --output-format json`，工具权限用 `--allowedTools "Bash(zloop:*)" ...` 精确放行或 `--dangerously-skip-permissions`；Codex 用 `codex exec --sandbox workspace-write -C <dir> --skip-git-repo-check`。

## 2. 为什么用 Rust，以及不为什么

- **要的**：单二进制、零运行时依赖、毫秒级启动（`next` 在每轮开头被调用，Python 版 import 开销 ≈ 60–100 ms）、长驻 runner 进程内存小、跨平台 `flock` 用 `fd-lock`。
- **不要的**：async 运行时（runner 是串行的"跑一轮 → 睡 → 再跑"，`std::thread::sleep` 足够）、数据库、插件系统、任何网络服务。
- 依赖控制在 7 个以内：`clap`（derive）、`serde` + `serde_json`、`chrono`、`anyhow`、`fd-lock`、`dirs`。

## 3. 数据模型：`state.json` 保持 v1，加可选字段

格式与 v0.1（Python 原型）**完全兼容**：Rust 只增加可选字段，未知键在读写时原样保留（`serde(flatten)`）。移除 Python 版之前曾实测两边交替操作同一文件无损（§12）。

```jsonc
{
  "version": 1,
  "goal":   { "id": "…", "text": "…", "status": "active|paused|done", "created_at": "…" },
  "policy": { "window_hours": 24, "max_runs": 60, "max_fail_streak": 3, "max_noop_streak": 3, "intervals_min": [3, 10, 30] },
  "todos":  [ { "id": "t1", "text": "…", "priority": 0, "status": "open|blocked|deferred|done",
                "blocked_by": [], "note": "", "updated_at": "…", "done_at": null } ],
  "ticks":  [ { "at": "…", "round": 1, "todo": "t1", "outcome": "done|progress|fail|block|noop|edit",
                "note": "…",
                // ---- Rust 版新增（全部可选）----
                "host": "claude|codex|cli",          // 谁在跑：由环境变量自动判定
                "session": "11111111-…",             // 宿主会话 id → resume
                "log": "log/20260827-054500-t1-done.md" } ],   // 本次执行留档（相对 .zloop/）
  "next_id": 2,
  "updated_at": "…"
}
```

会话 id 不单独建表——`zloop sessions` 从 ticks 去重派生（host, session, 首次/最后时间, tick 数, 涉及 todo）。这是 loopx "当前状态是事件的投影"原则的直接套用。

## 4. 目录布局

```
Cargo.toml
src/
  main.rs        入口：clap 解析 → 分派
  cli.rs         子命令定义与参数
  state.rs       加载/保存（tmp+fsync+rename）、fd-lock、find_root、默认策略      ← 对应 Python state.py
  todo.rs        [Pn] 解析、排序、executable、状态转移、loopx 导入                 ← todo.py
  tick.rs        decide()、streak、窗口计数、backoff、apply_done、to_json           ← tick.py
  prompt.rs      heartbeat 协议模板（三宿主）、STATE.md 渲染                         ← prompt.py
  hosts.rs       SKILL.md / openai.yaml 模板与幂等安装、Stop hook                   ← hosts.py
  session.rs     宿主与会话探测（CLAUDE_CODE_SESSION_ID / CODEX_THREAD_ID）、resume 命令、transcript 路径   [新]
  log.rs         执行留档写入与列表                                                  [新]
  context.rs     有界交接包                                                          [新]
  runner.rs      无头 runner：claude -p / codex exec，journal，循环与退避             [新]
tests/
  *.rs           集成测试（直接调库函数；CLI 用例通过 `CARGO_BIN_EXE_zloop` 调二进制）
docs/
```

体积预算：Rust 源码 ≤ 2,500 行（Python 版 921 行 × 约 2 的 Rust 膨胀 + 四个新模块），测试 ≤ 1,000 行。

## 5. 命令集

Python 版 8 个不变，新增 4 个。仍然每个 ≤ 4 个 flag。

| 命令 | 作用 | flags |
|---|---|---|
| `init "<goal>"` | 建状态 | `--force` |
| `plan` | 写有序 todo | `--add`, `--file`, `--replace`, `--from-loopx` |
| `next` | 该不该跑、跑哪条；**自动记录 host/session** | `--json`, `--peek` |
| `done <id>` | 唯一写回；**自动写 log 文件、记 host/session** | `--note`, `--outcome`, `--block`, `--next`；`--evidence <text\|@file>` 作为 log 正文（第 5 个 flag，仅此一例） |
| `edit <id>` | 改 todo | `--text`, `--status`, `--priority`, `--blocked-by` |
| `status` | 只读总览 | `--json`, `--md` |
| `heartbeat` | 每轮协议 | `--host` |
| `install` | 装 skill | `--claude`, `--codex`, `--claude-stop-hook` |
| **`sessions`** [新] | 列出出现过的宿主会话与 resume 命令 | `--host`, `--json` |
| **`context`** [新] | 有界交接包（给另一宿主/另一会话读） | `--budget <chars>`（默认 4000）, `--for claude\|codex` |
| **`log`** [新] | 列出/查看执行留档 | `--todo <id>`, `--last <n>`, `--show <file>` |
| **`run`** [新] | 前台无头 runner | `--host`（默认 claude）, `--max-rounds`, `--fast`, `--allow-all`, `--resume`, `--timeout-min`, `--exit-on-wait`, `--max-budget-usd` |
| **`start` / `stop`** [新，2026-08-27] | 后台 runner：`start` 以 setsid 分离重执行 `run`，stdio 到 `.zloop/runner/console.log`，pid 到 `.zloop/runner/pid`；`stop` SIGTERM→SIGKILL | 与 `run` 相同 |

`hook-stop` 保留为内部命令。

### 5.1 `phase`：循环现在到哪了（2026-08-27 补充）

用户实际使用时的第一个问题是"循环到底跑到哪一步了"。loopx 有这信息但散在 `lifecycle_phase` / `waiting_on` / `quota.state` / `scheduler_hint.execution_phase` / turn journal 五处；zloop 合成**一行**，出现在 `status` 第二行、`next --json` 的 `phase` 字段（第 10 个字段）、`context` 的目标段：

| 优先级 | 来源 | 输出 |
|---|---|---|
| 1 | `state.in_progress`（`next` 非 peek 交出 todo 时写入，`done` 清除；runner 开轮/收轮同样维护） | `executing t3 · round 4 · since 06:20 (3m ago) · host claude · via next\|runner` |
| 2 | runner journal 最后一条 `sleep`（新增事件，含 `until`） | `runner sleeping until 06:41 (2m10s left) · reason ready` |
| 2 | runner journal 悬空 `begin` | `runner round 4 on t2 since … — no end recorded (process may have died)` |
| 3 | `decide()` | `idle · next would run t4 …` / `waiting (user_gate) · retry in 10 min` / `stopped (done)` |

`in_progress` 是 `state.json` 新增的可选顶层键（`{todo, started_at, round, via, host, session}`），旧文件没有它照常读取。

### 5.2 开源借鉴（2026-08-27，详见 `OPEN-SOURCE-REVIEW.md`）

| 借自 | 加了什么 |
|---|---|
| Anthropic long-running harness | todo `acceptance`（`plan` 行 `文本 :: 验收`、`edit --acceptance`；heartbeat 要求自检；`done` 缺 evidence 提醒）、policy `preflight_cmd`（每轮前自检）、`run --git-commit`（每轮 checkpoint） |
| OpenHands 三道闸 | tick `cost_usd / num_turns / duration_ms`（来自 `claude -p` JSON）、policy `max_total_usd` → `stopped (budget)` |
| Beads | `zloop remember` + `.zloop/NOTES.md` + `context` 经验段；`zloop compact --keep-days` |
| Codex `/goal pause\|resume` | `zloop pause` / `zloop resume` |
| Ralph（Stop hook 反思） | 发现并修复：runner 拉起的 `claude -p` 会加载我们的 Stop hook——子进程设 `ZLOOP_RUNNER=1`，`hook-stop` 放行 |
| 没人做好的一件事 | policy `notify_url` / `notify_cmd`；runner 等人、限流、停机时通知；`zloop notify` |

## 6. 会话追踪与 resume（G4）

`session.rs`：

```rust
pub struct HostSession { pub host: Host, pub session: Option<String> }
pub enum Host { Claude, Codex, Cli }

pub fn detect() -> HostSession {
    if let Ok(id) = env::var("CLAUDE_CODE_SESSION_ID") { return Claude(id) }
    if let Ok(id) = env::var("CODEX_THREAD_ID")        { return Codex(id) }
    Cli
}
pub fn resume_command(host, session) -> Option<String>   // "claude --resume <id>" / "codex resume <id>"
pub fn transcript_path(host, session, project_root) -> Option<PathBuf>
// claude: ~/.claude/projects/<root 路径 '/'→'-'>/<id>.jsonl
// codex : glob ~/.codex/sessions/**/rollout-*-<id>.jsonl
```

- `next` 与 `done` 每次都调用 `detect()`，把结果写进 tick。`zloop run` 驱动时由 runner 从 `claude -p` 的 JSON 输出（`session_id`）/ `codex exec --json` 的 `thread.started` 事件里取 id，设置到子进程环境或写回 tick。
- `zloop sessions` 输出：

```
host    session                               ticks  first → last              todos      resume
claude  11111111-2222-3333-4444-555555555555  4      08-27 05:10 → 05:38       t1,t2      claude --resume 11111111-2222-3333-4444-555555555555
codex   019bd3f2-…                            1      08-27 05:41 → 05:41       t3         codex resume 019bd3f2-…
```

并标注 transcript 文件是否存在（`✓` / `missing`）。

## 7. 执行留档（G3）

`log.rs`：`done` 时写 `.zloop/log/<YYYYmmdd-HHMMSS>-<todo>-<outcome>.md`：

```markdown
# t2 · progress · 2026-08-27 05:38:12 +08:00

- goal: 把 demo 服务的启动时间降到 1 秒以内
- todo: [P0] 找出最慢的 3 个初始化步骤
- outcome: progress   round: 2
- host: claude   session: 11111111-…   resume: `claude --resume 11111111-…`
- note: 已定位 2 个，第 3 个待查

## Evidence
<--evidence 的正文；@file 时为文件内容；没有则省略此节>
```

tick 里记 `log` 相对路径；`zloop log` 按时间倒序列出（可按 todo 过滤），`--show` 打印一份；`status --md` 每条 tick 附 log 链接与 resume 命令。`noop` tick 不写文件（避免刷屏），`block` 写。

## 8. 跨宿主上下文（G2）

三层，全部继承 loopx 的分层思想：

1. **状态文件是唯一真源**——两个宿主读写同一个 `.zloop/state.json`，本身就"共享上下文"。
2. **`zloop context`** = loopx handoff packet 的简化版，预算默认 4000 字符（Codex `/goal` 上限），按优先级填充、超预算从尾部段落起删：
   ```
   ## 目标            goal.text
   ## 当前判断        最近 3 条非 noop tick 的 note（这是"我们现在相信什么"）
   ## 下一条          next 的 todo
   ## 待办            open todo 前 5 条（含 blocked 及其问题）
   ## 会话            每宿主最近一个 session + resume 命令
   ## 怎么继续        --for 指定宿主的续跑方式（同 heartbeat 尾句）
   ```
3. **SKILL.md 第一步改为 `zloop context`**（而不是 `zloop status`）：任何宿主、任何新会话进来先读交接包，再 `next`。

切换步骤（用户视角）：在 Claude Code 里 `/zloop` 做了两轮 → 打开 Codex，`$zloop` → Codex 先 `zloop context` 看到"当前判断 / 下一条 / Claude 那边的 resume 命令" → 继续 `next → 做 → done`，tick 上 host=codex。回到 Claude Code 时 `zloop sessions` 能看到 Codex 那一轮的 id，`codex resume <id>` 可回看细节。

## 9. 无头 runner（G1）

`runner.rs`，对应 loopx `turn run-once` + `loop_controller`，但去掉 7 阶段 settlement：

```rust
loop {
    let d = tick::decide(&state, now);
    if !d.should_run {
        match d.interval_min { None => break (reason), Some(m) => sleep(m) ; continue }
    }
    journal.begin(round, host, todo)                       // .zloop/runner/journal.jsonl（append-only）
    let prompt = prompt::heartbeat(&state, host) + "\n当前 todo：" + todo + "\n本轮结束前必须运行 writeback 命令。";
    let sid    = if resume { sessions::last(host) } else { None };
    let out    = match host {
        Claude => cmd!("claude", "-p", prompt, "--output-format", "json",
                       ["--resume", sid]?, "--allowedTools", "Bash(zloop:*),Read,Edit,Write,Glob,Grep",
                       allow_all.then("--dangerously-skip-permissions")),   // 解析 JSON: session_id / is_error / result
        Codex  => cmd!("codex", "exec", ["resume", sid]?, "--json", "-C", root, "--skip-git-repo-check",
                       "--sandbox", allow_all ? "danger-full-access" : "workspace-write",
                       "--output-last-message", tmp, "-") <<< prompt,      // 解析 JSONL: thread.started.thread_id
    };
    // 结算：模型是否真的写回了？
    let wrote_back = state_reloaded.ticks.len() > before && last_tick.round/outcome 属于本轮;
    if !wrote_back { tick::record(fail, todo, "runner: no writeback from host") }  // 计入 fail_streak → 3 次停
    patch last tick with host/session if missing
    journal.end(round, exit_code, session)
    if rounds >= max_rounds { break }
    sleep(d.interval_min)          // --fast: 分钟→秒，供 demo
}
```

- **幂等 / 崩溃恢复**：journal 每轮两行（begin / end）。启动时若最后一行是 begin 无 end → 上次进程在宿主执行中被杀；不重放（模型可能已 `done`），只在 tick 里记一条 `edit` 类型注释"runner restarted"，然后正常 `decide`（若模型已 done，next 自然指向下一条；若没写回，本轮会算 fail）。这比 loopx 的 6 阶段 checkpoint 粗，但满足"不重复 tick、能续"。
- **停机条件**全部来自 `decide()`：unplanned（一条 todo 都没有）/ all_done（有过 todo 全了结）/ user_gate / fail_streak / noop_streak / throttled(interval=None)，runner 不另设一套。
- **权限**默认最小：只放行 `Bash(zloop:*)` + 读写编辑；`--allow-all` 才 `--dangerously-skip-permissions` / `danger-full-access`。

## 10. 与 loopx 的对应关系（照搬清单）

| loopx 机制 | zloop-rs 落法 |
|---|---|
| ACTIVE_GOAL_STATE.md 作为 current-belief | `context` 的"当前判断"段 = 最近 3 条 tick note |
| runs/index.jsonl 事件账本 | `ticks[]`（含 host/session/log） |
| runs/`<ts>.md` 人读证据 | `.zloop/log/*.md` |
| turn-sessions 记 codex session_id | tick.session + `sessions` 命令 |
| thread binding (host_surface, thread_id) | 环境变量直接探测，不做绑定表 |
| thin heartbeat 1900 字符 | `heartbeat` ≤ 1200 字符 |
| handoff packet 16 行 / 1800 字符按段删 | `context` 4000 字符按段删 |
| spend after validated writeback | `done` 之后才有 tick；runner 无写回记 fail |
| journal 六阶段重放 | journal 两行 begin/end + 重启检查 |
| outer_controller `turn run-once` | `run` |
| unchanged 3 次停 / fail streak | `noop_streak` / `fail_streak` |
| 多 agent claim/lease/gate、capability、supervisor | 不做 |

## 11. 实施顺序（对应 LoopX Todo）

1. **P0 设计**（本文档）。
2. **P0 核心移植**：`state / todo / tick / prompt / hosts / cli` 六个模块 + 与 Python 版 33 个用例等价的 cargo test；Python 与 Rust 交替读写同一 `state.json` 的互读测试。
3. **P1 会话追踪**：`session.rs` + `next/done` 记录 + `sessions` 命令；用本会话的 `CLAUDE_CODE_SESSION_ID` 实测。
4. **P1 上下文与留档**：`log.rs`、`context.rs`、`status --md` 增强、SKILL.md 首步改为 `context`。
5. **P1 runner**：`runner.rs`；demo：临时项目 3 条琐碎 todo，`run --host claude --fast --max-rounds 3`，再 `run --host codex --once` 验证跨宿主。
6. **P2 发布**：`cargo build --release`、README、Python vs Rust 启动/next 延迟对比（`hyperfine` 或循环 `time`）。

## 12. 实现结果（2026-08-27 实测）

| 项 | 结果 |
|---|---|
| 代码量 | `src/` 10 个模块 2,190 行；`tests/` 656 行（38 个用例：tick 14 / todo 7 / state 7 / cli 10）；Python 参照版 921 行 |
| 依赖 | clap、serde、serde_json、chrono、anyhow、fd-lock、dirs（7 个，均为纯 Rust） |
| 二进制 | `target/release/zloop` 1.2 MB（lto + strip） |
| 性能 | `next --peek --json` 20 次均值：**Rust 12.4 ms / Python 52.8 ms**（4.3×） |
| 互读 | Rust `init/plan` → Python `next/done` → Rust `done/status/sessions` 在同一 `state.json` 上无损；Python 写的 tick 没有 host/session 字段，Rust 正常读取 |
| G1 长时运行 | `run --host claude --fast --max-rounds 2`：92 秒跑完 2 轮，均写回；伪造悬空 journal 后重启，打印 "previous run ended mid-round (round 3); continuing"，继续跑完 |
| G2 跨宿主 | 同一 demo 目录：t1、t2 由 Claude（`claude -p`）完成，t3 由 Codex（`codex exec`）完成；`zloop context` 同时列出两宿主的 resume 命令 |
| G3 留档 | 每轮生成 `.zloop/log/<ts>-<todo>-done.md`，含 host / session / resume / evidence |
| G4 resume | `zloop sessions` 输出 `claude --resume 11111111-…` 与 `codex resume 22222222-…`，两者 transcript 均 ✓ 存在；Claude 第 2 轮自动 `--resume` 了第 1 轮会话 |
| 安装 | 二进制装到 `~/.local/bin/zloop`，Python 控制台脚本已卸载（源码保留）；`/zloop` skill 已刷新为"先 `zloop context`" |

与 §4 布局的差异：模块与文件一一对应，无增删；`hook-stop` 保留；`done` 多了第 5 个 flag `--evidence`（设计已预告）。

## 13. 已知取舍

- 会话 id 靠环境变量：在非 Claude/Codex 的终端里手动跑 `zloop done`，host=cli、session 为空——这是事实，不伪造。
- runner 驱动的 `claude -p` 每轮是一次独立请求；会话续接策略 `--resume todo|all|none`，默认 **按 todo 谱系**（同一 todo 续接、换 todo 新会话，照搬 loopx 的 `(goal, agent, todo)` lineage），避免跨天单会话无限膨胀。
- 长程加固（2026-08-27，见 `LONG-RUN-AUDIT.md`）：每轮宿主超时 `--timeout-min`、等人时持续轮询而非退出、限流不计失败、`max_progress_streak`、`max_runs` 默认 480 且可关、`init --force` 归档、in_progress stale 标注、`--max-budget-usd`；runner 每次退出记 `stop`，启动时最后一条非 `stop` 即记 `restart`。
- 不做守护进程化（launchd / systemd）：`zloop run` 前台跑，需要后台就 `nohup` 或 tmux——loopx 的 launchd tick 那一套复杂度证明不值。
- Python 原型（v0.1）在 Rust 版验证通过后已从仓库移除（2026-08-27）；`docs/DESIGN.md` 保留为它的设计记录。
