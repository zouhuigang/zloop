# 与 loopx 的对比，以及明确不做的事

> 为什么是这个规模，以及哪些功能是有意不做的。
>
> ← 回到 [README](../../README.md)

| 维度 | loopx 0.5.2 | zloop 0.4 |
|---|---|---|
| 源码文件 / 行数 | 819 / 317,699（Python） | 23 / ≈10,245（Rust） |
| 顶层子命令 | 113（叶命令 307） | 29（含内部 `hook-stop`）——每条的用途见[命令详解](COMMANDS.md) |
| 单命令最多 flag | 75（`todo`） | 15（`run`） |
| `next` 一次调用 | 20–30 KB JSON，数百 ms | 10 个字段，12 ms |
| 状态存放处 | ≥ 9 处（两级 registry、Markdown 状态、runs/、turns/、leases/…） | 1 个 JSON（+ 可读的 log/*.md） |
| Todo 元数据字段 | ≈ 50（URL 编码塞进 Markdown 注释） | 8 |
| 每轮写回 | `refresh-state` + `spend-slot` + `todo complete`，顺序错一步丢账 | `zloop done` 一条（且强制留下技术文档） |
| 开始干活前 | 8–12 步事务、12 条 CLI 调用、必须注册 agent 身份 | `init` + `plan` |
| 宿主 prompt | ≈ 1,900 字符 / 7 条规则 / 含 `LOOPX_TURN` 环境变量 | ≈ 850 字符 / 5 条规则 |
| 无头运行 | `turn run-once`（仅 codex-cli，7 阶段结算） | `start` / `run`，任一宿主，journal 5 种事件 |
| 会话回看 | `turn-sessions/<sha>.json`（仅 headless codex） | 每 tick 记 host+session，`sessions` 直接给 resume 命令 |
| Claude Code 安装物 | 11 个 skill | 1 个 skill（+ 可选 Stop hook） |
| 运行时依赖 | 0（纯标准库 Python） | 无解释器 / 服务（静态单二进制；编译期 7 个 crate） |

## 明确不做

多 agent 协作、能力路由、仪表盘、聊天、飞书、事件溯源、全局跨项目 registry、PreToolUse 强制拦截、开机自启。两个 agent 同时跑同一个 `.zloop/` 不会写坏文件，但会互相覆盖进度——这是有意为之。

经验与约定也**不跨项目继承**（理由和"真要做的话最小长什么样"见[边界：经验和约定都不跨项目](COMMANDS.md#边界经验和约定都不跨项目这是取舍不是缺陷)）；
同理，[`zloop goals` 只看得见当前项目](MULTI-GOAL.md)——几个项目的目标没有一个合起来的视图。
