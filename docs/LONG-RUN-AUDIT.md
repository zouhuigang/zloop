# zloop 长程运行差距审计（对照 loopx）

> 目标：**指派一个长程任务后，zloop 能长时间运行而不出问题。** 本文逐项对照 loopx 为"长时运行"设计的机制（见 [`loopx-principles.md` §1 长时运行与上下文管理](loopx-principles.md#1-长时运行与上下文管理)），核对 zloop 0.2 的现状，给出风险等级与处置。
> 事实来源：zloop 源码（`src/runner.rs`、`tick.rs`、`state.rs`、`phase.rs`）与本机 `claude 2.1.247` / `codex-cli 0.147.0` 的 `--help`。
> 日期：2026-08-27。第 2 节的处置结果与第 3 节的 soak 实测由后续轮次追加。

## 1. 对照表

风险等级按"会不会让一个已指派的长程任务**停下来或跑坏**"来定：**高** = 会静默停摆或无限挂起；**中** = 会降质、烧钱或误停；**低** = 体验/可观测问题。

| # | 维度 | loopx 的做法 | zloop 0.2 现状 | 风险 | 处置 |
|---|---|---|---|---|---|
| 1 | **宿主挂死** | headless `codex exec` 有 115s 超时（`codex_cli.py:392`），超时保存会话供 resume | runner 用 `Command::output()` 等宿主，**无超时**；`claude -p` / `codex exec` 卡住 → 整个循环永久挂起，phase 一直 `executing` | **高** | t2：每轮超时（默认 30 min，`--timeout-min`），超时 kill 子进程、记 `fail` tick "host timed out"、继续下一轮 |
| 2 | **等人时的循环所有权** | user_gate → 通知后退避 30→120 min，3 次无变化停；Codex App automation 由宿主保活 | `decide` 返回 `interval=None`（user_gate/blocked 且 noop 耗尽、或 fail_streak）时 runner **直接退出**。用户几小时后解答了问题，没有任何东西再拉起循环 | **高** | t2：runner 区分"等人"（user_gate/blocked → 以最大间隔持续轮询，`--exit-on-wait` 可关）与"终态"（done/paused/fail_streak/progress_streak → 退出）。轮询只调 `decide`，不调模型，零成本 |
| 3 | **配额窗口** | `allowed = window_h·60/slot_min·compute`，默认 1440/天（等于不限） | `max_runs=60/天`。3 分钟一轮的真长程任务 3 小时就撞顶，之后 `throttled` 最长等 24h | **高** | t2：默认改为 480（= 全天 3 分钟一轮），并支持 `0` = 不限；保留为"防空转刹车"而非节奏控制 |
| 4 | **会话复用与上下文膨胀** | headless 按 `(goal, agent, todo)` 谱系复用 codex 会话，**换 todo 即新会话**（`codex_cli.py:65-80`）；Codex App 每 tick 新会话 | runner 取"该宿主最近一个 session"永远 `--resume`，跨 todo 跨天累积，上下文只增不减 → 变慢、变贵、compaction 丢细节 | **中** | t2：改为按 todo 谱系——同一 todo 续接、换 todo 开新会话（旧会话 id 仍在 tick 里可回看）；`--resume-all` 保留旧行为 |
| 5 | **停滞检测** | typed progress 指纹重复 2 次即 replan 义务；surface_only 连 3 次 focus_wait；todo 链 ≥15 触发 | 只有 `fail_streak`（3）与 `noop_streak`（3）。模型每轮报 `--outcome progress` 但永远不 done → **无限循环** | **中** | t2：新增 `max_progress_streak`（默认 8）：同一 todo 连续 progress 达阈值 → `stopped (progress_streak)`，等人拆分 |
| 6 | **宿主侧限流/过载** | Codex App 由宿主 backoff；loopx 无专门处理 | `claude -p` 返回 rate limit / overloaded → 记 `fail` → 3 次即 `fail_streak` 停。限流是**暂时的**，却被当成任务失败 | **中** | t2：识别 429/rate limit/overloaded/capacity 类错误 → 不记 fail，journal `sleep reason=host_rate_limited`，退避 30 min 重试 |
| 7 | **崩溃/中断恢复** | journal 6 阶段重放、幂等收据、`resume_session` | journal `begin/end` + 重启检测（已验证：悬空 begin → 提示并从当前状态续）；`done` 幂等拒重复 | 低 | 已达标；t3 soak 再验 kill -9 |
| 8 | **重复 tick / 双写** | turn_instance_id 幂等收据 | 写回是 `done` 单命令；runner 结算只看"本轮新增 tick"，宿主已写回则不再记 fail | 低 | 已达标 |
| 9 | **prompt 预算** | thin 1900 字符，硬顶 4000 | heartbeat ≤1200，context ≤4000 按段裁剪 | 低 | 已达标 |
| 10 | **历史增长** | runs/ 双写 + Completed Work Archive（done>12 归档） | `ticks[]` 无限增长（每轮 1–2 条，1 万轮 ≈ 数 MB，Rust 解析仍 ms 级）；`.zloop/log/` 一轮一文件；journal 无限追加；**换目标 `init --force` 直接覆盖旧状态** | 低（增长）/ 中（丢历史） | t2：`init --force` 自动归档旧 `state.json` 到 `.zloop/archive/`；ticks/log 压缩留 P2 |
| 11 | **in_progress 悬挂** | lease TTL 45 min | 交互式 `next` 交出 todo 后会话死掉 → phase 永远 `executing`；下一次 `next` 会覆盖，但中间误导人 | 低 | t2：phase 对超过 `stale_after_min`（默认 120）的 in_progress 标注 `stale`，不自动清除 |
| 12 | **runner 进程自身存活** | launchd 一次性 tick（作者自己评价"不值"）；Codex App automation 由 App 保活 | 前台进程；终端关闭即死。重启后靠 journal 续 | 中 | **已做**：`zloop start` 把 runner 以独立会话（setsid）放到后台，关终端不受影响；pid 在 `.zloop/runner/pid`，`zloop status` 显示是否在跑，`zloop stop` 停。**不做**开机自启/崩溃自动拉起：电脑重启后再 `zloop start` 一次即可（幂等续跑） |
| 13 | **单轮预算** | quota slot 计分钟 | 一轮花多少钱没有上限 | 中 | t2：`--max-budget-usd`（透传给 `claude -p`，默认不设）；Codex 无对应参数，写明 |
| 14 | **退避节奏** | `[i,2i,4i]`，Codex App 顶 60 min | `[3,10,30]` 三档 | 低 | 已达标 |
| 15 | **写回顺序坑** | refresh → spend → complete 顺序错即丢账 | `done` 一条命令 | 低 | 优于 loopx |
| 16 | **模型不写回** | typed 指纹 + replan 义务 | runner 结算：无新 tick → 记 fail；3 次停 | 低 | 已达标；与 #6 区分开限流 |

## 2. P0 处置清单（进入 t2）

按对长程任务的威胁排序：

1. **#1 宿主超时**：`run --timeout-min <N>`（默认 30；`--fast` 时按秒）；超时 → kill、记 `fail`（note 含 timed out）、journal end 标 `timed_out=true`。
2. **#2 等人不退出**：runner 在 `user_gate` / `blocked` 下按退避阶梯的**末档**（`intervals_min` 末项，默认 30 min；`tick::ladder_tail`）持续轮询；`--exit-on-wait` 恢复旧行为。终态（done / paused / fail_streak / progress_streak）才退出。
3. **#3 配额默认**：`max_runs` 默认 480；`0` = 不限。
4. **#4 会话谱系**：同一 todo 续接上次会话，换 todo 新会话；`--resume-all` 保留"一直续接"。
5. **#5 progress_streak**：policy 新增 `max_progress_streak`（默认 8），`decide` 新增停机原因 `progress_streak`。
6. **#6 限流识别**：`claude -p` 的 `is_error` + 错误文本匹配 `rate limit|overloaded|429|capacity|quota` → 不记 fail，睡 30 min 重试；journal 记 `sleep reason=host_rate_limited`。
7. **#10 归档**：`init --force` 前把旧 `state.json` 移到 `.zloop/archive/<created_at>-<goal.id>.json`。
8. **#11 stale 标注**：phase 对 in_progress 超过 `stale_after_min`（默认 120）加 `⚠ stale`。
9. **#13 单轮预算**：`run --max-budget-usd <x>` 透传（仅 claude）。

每项附 cargo test；#1/#2/#4/#6 用假宿主在 t3 soak 里做端到端验证。

### 处置结果（t2，2026-08-27）

| 项 | 实现 | 测试 |
|---|---|---|
| #1 宿主超时 | `run --timeout-min <N>`（默认 30；`--fast` 按秒）。子进程 stdout/stderr 用线程排空，主线程 200ms 轮询 `try_wait`，到点 `kill` → 记 `fail`（note "host timed out after …"），journal `end.timed_out=true`，`in_progress` 清除 | `runner_test::hung_host_is_killed_and_recorded_as_fail`（假宿主 sleep 30，1s 超时） |
| #2 等人不退出 | `wait_plan()`：`interval=None` 且 reason ∈ {user_gate, blocked} → 以 `intervals_min` 末项持续轮询（journal `sleep reason="user_gate (polling until a human unblocks)"`）；`--exit-on-wait` 恢复退出。终态（done/paused/fail_streak/progress_streak）仍退出 | `waiting_on_a_human_polls_instead_of_exiting`（2.5s 后另一"终端" `edit --status open`，runner 自动续跑到 done） |
| #3 配额默认 | `max_runs` 默认 480；`0` 关闭窗口刹车 | `tick_test::max_runs_zero_disables_the_window_brake` |
| #4 会话谱系 | `--resume todo\|all\|none`（默认 todo）：`pick_session` 按 (host, todo) 取最近**写回**会话（`tick::is_writeback`，A-19 之后；人敲 `feedback`/`edit`/`next` 留下的 session 不算数）；换 todo 开新会话 | `sessions_follow_todo_lineage_by_default`（三种模式的 argv 断言）、`a_humans_feedback_is_not_a_session_to_resume` |
| #5 progress_streak | policy `max_progress_streak`（默认 8，0 关闭）；`decide` 对候选 todo 的连续 progress 计数，达阈值 → `stopped (progress_streak)`；换 todo 或 done/fail/block 打断 | `tick_test::progress_streak_on_one_todo_stops_the_loop` |
| #6 限流识别 | `is_error`/非零退出 且文本含 `rate limit / 429 / overloaded / capacity / quota / too many requests / usage limit` → 不记 tick、journal `end.rate_limited=true` + `sleep reason=host_rate_limited`、睡阶梯末档（`tick::ladder_tail`）后重试；该轮不计入 `--max-rounds` | `rate_limit_is_not_a_failure_and_is_retried` |
| #10 归档 | `init --force` 先把旧 `state.json` 移到 `.zloop/archive/<created_at>-<goal.id>.json`（同名加序号），日志目录不动 | `cli_test::init_force_archives_the_previous_goal` |
| #11 stale 标注 | policy `stale_after_min`（默认 120，0 关闭）；phase 对超龄 `in_progress` 追加 `⚠ stale (>120m, …)` | `cli_test::stale_in_progress_is_flagged` |
| #13 单轮预算 | `run --max-budget-usd <x>` 透传 `claude -p --max-budget-usd`（Codex 无对应参数） | `max_budget_flag_is_passed_to_claude` |

全量 `cargo test`：48 通过。`run` 的 flag 数升到 8（host / max-rounds / fast / allow-all / resume / timeout-min / exit-on-wait / max-budget-usd），是"单命令 ≤4 flag"的唯一例外——runner 是唯一需要与外部宿主打交道的命令，参数都对应真实故障模式，不做合并。

## 3. soak 实测（t3 追加）

### 结果（14/14 通过，2026-08-27 07:09）

场景：12 条 todo；假宿主第 5 次调用返回 429、第 7 次不写回、第 9 次挂死 10s（超时 2s）；runner 启动 4s 后（第 3 轮执行中）`kill -9` 再重启；策略间隔 [1,1,2]s（`--fast`）。之后另建项目跑一轮真实 `claude -p`。

| 检查项 | 期望 | 实际 | 结果 |
|---|---|---|---|
| runner 在 kill -9 后重启并自行跑完（≤180s） | `True` | `True` | ✅ |
| 12 条 todo 全部 done，goal=done | `(12, 'done')` | `(12, 'done')` | ✅ |
| 每条 todo 恰好一个 done tick（无重复） | `['t1', 't10', 't11', 't12', 't2', 't3', 't4', 't5', 't6', 't7', 't8', 't9']` | `['t1', 't10', 't11', 't12', 't2', 't3', 't4', 't5', 't6', 't7', 't8', 't9']` | ✅ |
| 超时轮记 fail（timed out） | `1` | `1` | ✅ |
| 不写回轮记 fail（without writing back） | `1` | `1` | ✅ |
| 限流轮不记 tick、journal 有 host_rate_limited 睡眠 | `True` | `True` | ✅ |
| journal 有 restart 事件（kill -9 被识别） | `True` | `True` | ✅ |
| journal begin 数 = end 数 + 悬空(≤1) | `True` | `True` | ✅ |
| 所有 done tick 带 host=claude 与 session | `True` | `True` | ✅ |
| runner 日志出现 TIMED OUT / rate-limited / NO WRITEBACK 三种处理 | `(True, True, True)` | `(True, True, True)` | ✅ |
| 最终 phase = stopped (done) | `True` | `True` | ✅ |
| kill -9 时刻 / 当时 phase | `-` | `4.0s / idle · next would run t4 [P1] task 4` | ℹ️ |
| 假宿主总调用次数 / 总耗时 | `-` | `15 次 / 29s` | ℹ️ |
| tick 总数（done/fail） | `-` | `14（12/2）` | ℹ️ |
| 真实 claude -p 一轮写回（--max-budget-usd 1.00, --timeout-min 300） | `True` | `True` | ✅ |
| soak.txt 内容正确 | `soak ok` | `soak ok` | ✅ |
| 真实会话 transcript ✓，可 claude --resume | `True` | `True` | ✅ |
| 真实一轮耗时 | `-` | `38s` | ℹ️ |

runner 日志节选：
```
runner: round 1 → t1 [claude]
runner: round 1 written back · done t1
runner: session → claude --resume soak-t1
runner: round 2 → t2 [claude]
runner: round 2 written back · done t2
runner: session → claude --resume soak-t2
runner: round 3 → t3 [claude]
runner: round 3 written back · done t3
runner: session → claude --resume soak-t3
runner: previous run did not stop cleanly (last event: sleep); continuing from current state
runner: round 4 → t4 [claude]
runner: round 4 written back · done t4
runner: session → claude --resume soak-t4
runner: round 5 → t5 [claude]
runner: round 5 host rate-limited · not counted · sleeping 2 s · API Error 429: rate limit reached
runner: round 5 → t5 [claude]
runner: round 5 written back · done t5
runner: session → claude --resume soak-t5
runner: round 6 → t6 [claude]
runner: round 6 NO WRITEBACK (recorded fail) · forgot to write back
runner: session → claude --resume nowb
runner: round 6 → t6 [claude] resume nowb
runner: round 6 written back · done t6
runner: session → claude --resume soak-t6
runner: round 7 → t7 [claude]
runner: round 7 TIMED OUT (recorded fail) · 
runner: round 7 → t7 [claude]
runner: round 7 written back · done t7
runner: session → claude --resume soak-t7
runner: round 8 → t8 [claude]
runner: round 8 written back · done t8
runner: session → claude --resume soak-t8
runner: round 9 → t9 [claude]
runner: round 9 written back · done t9
runner: session → claude --resume soak-t9
runner: round 10 → t10 [claude]
runner: round 10 written back · done t10
runner: session → claude --resume soak-t10
runner: round 11 → t11 [claude]
runner: round 11 written back · done t11
```


前两次运行分别暴露两件事，都已处理：
1. 第一次（12/14）`kill -9` 恰好落在挂死轮执行中，runner 先于超时被杀，"超时记 fail"未触发——时序碰撞而非缺陷；顺带验证了 runner 被杀后孤儿宿主进程即使稍后完成也不会造成重复（`done` 二次被拒）。故障时序已调整。
2. 第二次（13/14）`kill -9` 落在两轮之间的睡眠期，journal 最后一条是 `sleep`，旧的重启检测只认悬空 `begin`，于是静默重启。已改为：runner 每次退出记 `stop`，启动时最后一条不是 `stop` 即记 `restart`（含 `after` 字段）。

**结论：在挂死、限流、不写回、runner 被 kill -9 四种故障叠加下，12 条 todo 各恰好完成一次，无重复 tick，runner 自行恢复并跑到 `stopped (done)`。**
