# zloop 文档

[README](../README.md) 只讲「干什么、怎么装、怎么用」。这里是其余的部分，按用途分三层。

## [`guide/`](guide/) — 怎么用得更深

| 文档 | 讲什么 |
|---|---|
| [COMMANDS.md](guide/COMMANDS.md) | 命令详解：每条干什么、什么时候敲、参数和实测例子 |
| [STATUS-GALLERY.md](guide/STATUS-GALLERY.md) | `zloop status` 八种状态各自长什么样、页脚给哪几条命令 |
| [TECH-DOCS.md](guide/TECH-DOCS.md) | 每条 todo 留一份技术文档，别人接手时读得懂 |
| [MULTI-GOAL.md](guide/MULTI-GOAL.md) | 一个项目多个目标：停放、切换、归档，以及边界 |
| [TUNING.md](guide/TUNING.md) | `policy` 七个字段 + runner 参数各控制什么 |
| [INTERNALS.md](guide/INTERNALS.md) | `next` 的决策梯、`.zloop/` 目录与 `state.json` 结构 |
| [VS-LOOPX.md](guide/VS-LOOPX.md) | 与 loopx 的对比，以及明确不做的事 |
| [MIGRATE-FROM-LOOPX.md](guide/MIGRATE-FROM-LOOPX.md) | 把 loopx 的状态导进来 |
| [DEVELOPMENT.md](guide/DEVELOPMENT.md) | 构建、测试、格式闸，以及写文档的约定 |

## [`design/`](design/) — 为什么这么做

| 文档 | 讲什么 |
|---|---|
| [DESIGN.md](design/DESIGN.md) | 整体设计：状态模型、调度、写回 |
| [RUST-DESIGN.md](design/RUST-DESIGN.md) | 从 Python 重写成 Rust 的取舍 |
| [ADAPTIVE-REPLAN.md](design/ADAPTIVE-REPLAN.md) | 自适应重规划：做完一条就重估后续，以及监督回路 |
| [SELF-IMPROVEMENT.md](design/SELF-IMPROVEMENT.md) | 自我改进回路：反馈、统计、回看、约定与经验两层 |
| [KEEP-AWAKE.md](design/KEEP-AWAKE.md) | 合盖不休眠是怎么做到的，以及为什么要 sudoers |
| [loopx-principles.md](design/loopx-principles.md) | loopx 的原则里哪些留下了 |
| [loopx-scheduling-notes.md](design/loopx-scheduling-notes.md) | loopx 调度部分的阅读笔记 |

## [`audit/`](audit/) — 做过什么检查、查实了什么

| 文档 | 讲什么 |
|---|---|
| [FINDINGS.md](audit/FINDINGS.md) | **42 条确认缺陷清册**，每条带可复现步骤与处置 |
| [CODE-AUDIT.md](audit/CODE-AUDIT.md) | 全量代码审查的过程与证据 |
| [LONG-RUN-PROOF.md](audit/LONG-RUN-PROOF.md) | 长程运行实证：4 小时 17 轮 0 人工介入，判据取自 zloop 之外 |
| [LONG-RUN-AUDIT.md](audit/LONG-RUN-AUDIT.md) | 长程加固审查 |
| [GOALS-REVIEW.md](audit/GOALS-REVIEW.md) | 多目标模块审查 |
| [STATUS-REVIEW.md](audit/STATUS-REVIEW.md) | `status` 界面审查 |
| [TEST-REPORT.md](audit/TEST-REPORT.md) | 测试报告 |
| [OPEN-SOURCE-REVIEW.md](audit/OPEN-SOURCE-REVIEW.md) | 同类开源方案调研，借鉴了什么 |

---

写文档的约定见 [DEVELOPMENT.md](guide/DEVELOPMENT.md)：**跨文档引用一律写成锚点链接**，
`scripts/check-doc-links.py` 会当闸拦住（`cargo test` 里也有一条）。
