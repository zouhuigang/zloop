# zloop 功能自测报告

> 目标：验证 zloop 各功能是否正常，按 loopx 核心功能对照。由 zloop 自己调度（本仓库 `.zloop/`），每轮一条 todo，每节由对应 todo 的驱动脚本生成。
> 日期：2026-08-27　zloop 0.2.0

## 1. 基础循环（t1）— 31/31 通过

对应 loopx：`quota should-run` 状态梯、`todo add/update/complete`、`refresh-state`+`spend-slot` 写回。用真实二进制 `zloop 0.2.0` 在临时目录逐场景执行。

| 检查项 | 期望 | 实际 | 结果 |
|---|---|---|---|
| ready: 选最高优先级且按写入顺序 | `('ready', True, 't2', 3)` | `('ready', True, 't2', 3)` | ✅ |
| ready: next --json ≤10 字段 | `True` | `True` | ✅ |
| init 已存在时拒绝（exit 1） | `True` | `True` | ✅ |
| blocked: 依赖未满足 → blocked, interval 10 | `('blocked', 10)` | `('blocked', 10)` | ✅ |
| blocked: 2 次 noop 后退避 30 | `30` | `30` | ✅ |
| noop_streak: 3 次 noop 后 interval null（停） | `None` | `None` | ✅ |
| noop 记录进 ticks（peek 不记） | `['noop', 'noop', 'noop']` | `['noop', 'noop', 'noop']` | ✅ |
| edit 打断 noop_streak → ready t1 | `('ready', 't1')` | `('ready', 't1')` | ✅ |
| 依赖完成后 t2 可执行 | `t2` | `t2` | ✅ |
| user_gate 不阻塞其它可执行 todo（跳到 t2） | `('ready', 't2')` | `('ready', 't2')` | ✅ |
| 只剩 blocked(user) → user_gate | `('user_gate', None)` | `('user_gate', None)` | ✅ |
| block 写入 blocked_by=['user'] 且 note=问题 | `(['user'], 'which db?')` | `(['user'], 'which db?')` | ✅ |
| fail_streak: 连续 3 次 fail → 停 | `('fail_streak', None, False)` | `('fail_streak', None, False)` | ✅ |
| edit（人介入）重置 fail_streak → ready | `ready` | `ready` | ✅ |
| throttled: 窗口内已达 max_runs | `throttled` | `throttled` | ✅ |
| throttled: interval ≈ 24h 后释放（分钟） | `True` | `True` | ✅ |
| 最后一条完成 → goal.status=done | `done` | `done` | ✅ |
| goal done → reason done, 停 | `('done', None)` | `('done', None)` | ✅ |
| active 但无开放 todo → all_done | `all_done` | `all_done` | ✅ |
| paused → reason paused, 停 | `('paused', None, False)` | `('paused', None, False)` | ✅ |
| plan 追加后 goal 从 done 回到 active（paused 保持） | `paused` | `paused` | ✅ |
| done --next 在其后插入同/指定优先级后继 | `['t1', 't3', 't2']` | `['t1', 't3', 't2']` | ✅ |
| 后继优先级取自 [P1] | `1` | `1` | ✅ |
| 重复 done 被拒（exit 2） | `True` | `True` | ✅ |
| 未知 id 被拒（exit 2） | `True` | `True` | ✅ |
| plan --replace 只清未完成项、保留 done | `[('t1', 'done'), ('t4', 'open')]` | `[('t1', 'done'), ('t4', 'open')]` | ✅ |
| status 首行显示 goal 与状态 | `True` | `True` | ✅ |
| status --md 以 # zloop 开头 | `True` | `True` | ✅ |
| --dir 指向子目录时向上找到 .zloop（find_root） | `basic loop test` | `basic loop test` | ✅ |
| 无状态目录 → exit 1 + 明确提示 | `(1, True)` | `(1, True)` | ✅ |
| plan --from-loopx 只导入未勾选项并保留 [Pn] | `t2 [P0] open one` | `t2 [P0] open one` | ✅ |

## 2. 宿主接入（t2）— 25/25 通过

对应 loopx：`heartbeat-prompt --thin`（1900 字符预算）、`slash-commands --install`（managed 标记 + readback）、`claude_goal_mode` 的 PreToolUse hook（这里换成 Stop hook）。

| 检查项 | 期望 | 实际 | 结果 |
|---|---|---|---|
| heartbeat --host claude ≤1200 字符（loopx thin 是 1900） | `True` | `True` | ✅ |
| heartbeat --host claude 含 5 条协议与宿主尾句 `/loop /zloop` | `True` | `True` | ✅ |
| heartbeat --host codex-app ≤1200 字符（loopx thin 是 1900） | `True` | `True` | ✅ |
| heartbeat --host codex-app 含 5 条协议与宿主尾句 `automation_update` | `True` | `True` | ✅ |
| heartbeat --host codex-cli ≤1200 字符（loopx thin 是 1900） | `True` | `True` | ✅ |
| heartbeat --host codex-cli 含 5 条协议与宿主尾句 `/goal` | `True` | `True` | ✅ |
| heartbeat 未知宿主被 clap 拒绝（exit 2） | `True` | `True` | ✅ |
| install 首次写入 4 个文件（wrote×4） | `4` | `4` | ✅ |
| Claude SKILL.md 有 frontmatter name=zloop | `True` | `True` | ✅ |
| Claude SKILL.md 带 managed 标记 | `True` | `True` | ✅ |
| Claude SKILL.md 首步是 zloop context | `True` | `True` | ✅ |
| Codex SKILL.md 尾句是 automation 续跑 | `True` | `True` | ✅ |
| Codex agents/openai.yaml allow_implicit_invocation=false | `True` | `True` | ✅ |
| settings.json 里 Stop hook = zloop hook-stop | `zloop hook-stop` | `zloop hook-stop` | ✅ |
| install 重复执行全部 kept（幂等） | `(4, 0)` | `(4, 0)` | ✅ |
| 覆盖非 managed 文件被拒绝 | `True` | `True` | ✅ |
| install 不带 flag 提示用法（exit 2） | `True` | `True` | ✅ |
| 本机 ~/.claude/skills/zloop/SKILL.md 存在且 managed | `True` | `True` | ✅ |
| 本机 ~/.codex/skills/zloop/SKILL.md 存在 | `True` | `True` | ✅ |
| 本机 settings.json 已含 Stop hook | `True` | `True` | ✅ |
| hook-stop 有可执行 todo → decision=block | `block` | `block` | ✅ |
| hook-stop reason 含协议与当前 todo | `True` | `True` | ✅ |
| hook-stop 只剩 user_gate → 放行（空输出） | `` | `` | ✅ |
| hook-stop 全部完成 → 放行 | `` | `` | ✅ |
| hook-stop 无 .zloop 目录、坏 JSON → 静默放行 exit 0 | `(0, '')` | `(0, '')` | ✅ |

## 3. 会话与留档（t3）— 21/21 通过

对应 loopx：`runs/<ts>.json+.md` 双写与 `Progress Ledger`（留档）、`turn-sessions/<sha>.json` 与 thread binding（会话）。

| 检查项 | 期望 | 实际 | 结果 |
|---|---|---|---|
| done 自动记录 host（claude/codex/cli） | `['claude', 'codex', 'cli']` | `['claude', 'codex', 'cli']` | ✅ |
| session 来自 CLAUDE_CODE_SESSION_ID / CODEX_THREAD_ID，cli 为空 | `['01119e7e-ff5b-4a34-b1df-61fad1afe2ca', 'thread-test-1', None]` | `['01119e7e-ff5b-4a34-b1df-61fad1afe2ca', 'thread-test-1', None]` | ✅ |
| noop tick 也带 host 字段 | `cli` | `cli` | ✅ |
| sessions 去重出 2 个宿主会话（cli 无 session 不列） | `['claude', 'codex']` | `['claude', 'codex']` | ✅ |
| sessions 给出可执行 resume 命令 | `['claude --resume 01119e7e-ff5b-4a34-b1df-61fad1afe2ca', 'codex resume thread-test-1']` | `['claude --resume 01119e7e-ff5b-4a34-b1df-61fad1afe2ca', 'codex resume thread-test-1']` | ✅ |
| 本会话 transcript 真实存在（~/.claude/projects/*/<id>.jsonl） | `True` | `True` | ✅ |
| sessions 记录 todos 与 tick 数 | `(['t1'], 1)` | `(['t1'], 1)` | ✅ |
| sessions --host codex 只列 codex | `True` | `True` | ✅ |
| sessions 文本输出标注 ✓ transcript | `True` | `True` | ✅ |
| 每个非 noop tick 生成一个 log 文件（3 个） | `3` | `3` | ✅ |
| log 文件名 <ts>-<todo>-<outcome>.md | `True` | `True` | ✅ |
| log 含 goal/todo/outcome/host/session/resume 行 | `True` | `True` | ✅ |
| tick.log 指向该文件（相对 .zloop/） | `True` | `True` | ✅ |
| --evidence @file 正文进入 ## Evidence | `True` | `True` | ✅ |
| zloop log 按时间倒序列出 4 条 | `4` | `4` | ✅ |
| zloop log --todo t2 只列 t2 的 2 条 | `2` | `2` | ✅ |
| zloop log --show <file> 打印正文 | `True` | `True` | ✅ |
| log --show 不存在 → exit 2 | `True` | `True` | ✅ |
| status --md 含 ## Sessions 与 resume 命令 | `True` | `True` | ✅ |
| status --md 每条 tick 附 [log] 链接 | `True` | `True` | ✅ |
| status 末行给出 last session resume | `True` | `True` | ✅ |

## 4. 跨宿主上下文（t4）— 22/22 通过

对应 loopx：handoff packet（16 行 / 1800 字符预算，超预算按段落逐级删）、ACTIVE_GOAL_STATE.md 的 current-belief 段、thin prompt 只指向状态不粘贴状态。

| 检查项 | 期望 | 实际 | 结果 |
|---|---|---|---|
| context 含段落 ## 目标 | `True` | `True` | ✅ |
| context 含段落 ## 当前判断 | `True` | `True` | ✅ |
| context 含段落 ## 下一条 | `True` | `True` | ✅ |
| context 含段落 ## 待办 | `True` | `True` | ✅ |
| context 含段落 ## 会话 | `True` | `True` | ✅ |
| context 含段落 ## 怎么继续 | `True` | `True` | ✅ |
| 当前判断 = 最近 3 次非 noop 执行（含 block） | `True` | `True` | ✅ |
| 下一条 = next 选中的 t2 | `True` | `True` | ✅ |
| 待办最多 5 条且标注 blocked 的问题 | `True` | `True` | ✅ |
| 会话段列出两宿主 resume 命令 | `True` | `True` | ✅ |
| 默认预算 4000 内 | `True` | `True` | ✅ |
| --for claude 尾句提到 /loop /zloop | `True` | `True` | ✅ |
| --for codex 尾句提到 zloop run --host codex | `True` | `True` | ✅ |
| --for cli 尾句是终端用法 | `True` | `True` | ✅ |
| --budget 300 → 长度 ≤301 | `True` | `True` | ✅ |
| 裁剪时优先保留 目标/当前判断/下一条 | `True` | `True` | ✅ |
| --budget 900 → 从尾部段落起删，仍保留『怎么继续』 | `True` | `True` | ✅ |
| 读取 Python 版 state：next 正常选 t2，round=1 | `('ready', 't2', 1)` | `('ready', 't2', 1)` | ✅ |
| Rust 写回后未知键 python_only_key 保留 | `{'kept': True}` | `{'kept': True}` | ✅ |
| 旧 tick 无 host 字段保持原样，新 tick 有 host/session/log | `(False, True)` | `(False, True)` | ✅ |
| 微秒时间戳 2026-…T23:00:30.123456+08:00 可解析（sessions/context 不报错） | `0` | `0` | ✅ |
| goal 全部完成后自动 done | `done` | `done` | ✅ |

## 5. 长时运行 runner（t5）— 18/19 通过

对应 loopx：`turn run-once`（codex exec 驱动 + 6 阶段 journal 重放）、`loop_controller` 六态、unchanged 3 次停、spend-after-writeback。

| 检查项 | 期望 | 实际 | 结果 |
|---|---|---|---|
| 真实 claude -p 一轮：runner 报告 written back | `True` | `True` | ✅ |
| alpha.txt 由宿主创建且内容正确 | `alpha ok` | `alpha ok` | ✅ |
| 宿主写回的 tick 带 host=claude 与真实 session id | `True` | `True` | ✅ |
| runner 打印 resume 命令 | `True` | `True` | ✅ |
| sessions 能看到 runner 产生的会话且 transcript ✓ | `True` | `True` | ✅ |
| journal 有 begin/end 一对且 wrote_back=true | `(['begin', 'end'], True)` | `(['begin', 'end'], True)` | ✅ |
| 真实一轮耗时（秒） | `-` | `77` | ℹ️ |
| 宿主不写回 → 每轮记 fail，3 次后 runner 停（fail_streak） | `True` | `True` | ✅ |
| 3 条 fail tick，note 说明 runner 检测到无写回 | `3` | `3` | ✅ |
| fail tick 记录假宿主返回的 session id | `fake-sess-1` | `fake-sess-1` | ✅ |
| todo 仍 open（fail 不改状态） | `open` | `open` | ✅ |
| 此时 next 也判 fail_streak 停 | `('fail_streak', None)` | `('fail_streak', None)` | ✅ |
| 悬空 begin → 启动时打印 previous run ended mid-round | `True` | `True` | ✅ |
| journal 追加 restart 事件后继续 begin/end | `['begin', 'restart', 'begin', 'end']` | `['begin', 'restart', 'begin', 'end']` | ✅ |
| 假宿主写回 2 轮后 runner 因 all done 停 | `True` | `True` | ✅ |
| 两条 todo 均 done，tick 由宿主写回并补上 session | `(['done', 'done'], ['fake-sess-2', 'fake-sess-2'])` | `(['done', 'done'], ['fake-sess-2', 'fake-sess-2'])` | ✅ |
| runner 子进程环境剥离了父会话的 CLAUDE_CODE_SESSION_ID（session 不是 should-be-stripped） | `False` | `False` | ✅ |
| goal 完成后 status=done | `done` | `done` | ✅ |
| 全部等人且 noop 已耗尽 → runner 立即 stop (user_gate) | `True` | `True` | ✅ |

## 总结

- 6 轮 / 6 条 todo 全部由 zloop 自身调度完成（本仓库 `.zloop/`），每轮一条；`zloop log` 可回看每轮留档，`zloop sessions` 可 `claude --resume` 回到本会话。
- 合计 **117 项检查通过，0 项失败**。
- 自测发现并修复 1 个 bug：runner 把 zloop 所在目录前置到子进程 PATH，会遮蔽同目录下用户的 `claude`/`codex`（t5 假宿主场景暴露），已改为追加到末尾（t6）。
- 覆盖的 loopx 核心功能：should-run 状态梯与写回、heartbeat/skill/hook 宿主接入、runs 留档与 turn-sessions、handoff packet、turn run-once/loop_controller。
- 未覆盖（zloop 明确不做或本次未实机验证）：多 agent claim/lease/gate、capability route、Codex App RRULE automation（只验证了 heartbeat 文本）、`run --host codex`（上一会话已实测 1 轮，本次未重复）。
