# 从 loopx 迁移

> 把 loopx 的状态导进 zloop。
>
> ← 回到 [README](../../README.md)

```bash
cd my-project
zloop init "$(grep '^objective:' .codex/goals/<goal>/ACTIVE_GOAL_STATE.md | cut -d'"' -f2)"
zloop plan --from-loopx .codex/goals/<goal>/ACTIVE_GOAL_STATE.md
```

只导入未勾选的 `- [ ] [Pn] …` 行（User Todo 与 Agent Todo 两节都算），剥掉 `<!-- loopx:todo … -->` 注释，`[P0]/[P1]/[P2]` 前缀原样保留；已完成 `[x]` 和延后 `[-]` 的不导入。loopx 的 `claimed_by`、`task_class`、`action_kind`、lease、successor 链等元数据没有对应物，直接丢弃。

---
