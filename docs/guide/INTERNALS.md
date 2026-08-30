# 内部：`next` 怎么决定、`.zloop/` 长什么样

> 调度的决策梯，以及磁盘上那几个文件各是什么。
>
> ← 回到 [README](../../README.md)

```
paused/done  >  unplanned / all_done  >  user_gate / blocked  >  fail_streak  >  progress_streak  >  throttled  >  ready
```

- 有可执行 todo（`open` 且 `blocked_by` 全部 done）→ `ready`，选 `(priority, 写入顺序)` 最靠前的一条，`interval_min = 3`。
- 一条 todo 都没有 → `unplanned`（去 `zloop plan`）；有过 todo 但全了结了 → `all_done`（去开新目标）。两者都是 `interval_min = null`，但下一步不一样，所以不共用一个词。
- 全部 blocked 且有人在等 → `user_gate`；纯依赖未满足 → `blocked`。退避 10 → 30 分钟，交互式连续 3 次 noop 后 `interval_min = null`。
- 最近连续 3 次 `fail` → `fail_streak`；同一 todo 连续 8 次 `progress` → `progress_streak`；两者都停下等人。
  停下之后人的任何一句话（`feedback` / 任意 `edit`）都重置；**还在跑的时候**只有 `edit` 改的正是那条 todo 才重置——
  人在另一个终端顺手整理 backlog 不该把这两道闸拆了（A-20 / A-21）。
- 24 小时窗口内记账满 `max_runs` → `throttled`，给出几分钟后释放。

**`noop` 计数只走交互式这一路。** `noop` tick 只有 `zloop next` 在 `should_run=false` 时会记（`--peek` 不记），runner 在等待那一支只写 journal 的 `sleep`，一条 tick 都不记。所以上面两处「连续 3 次 noop 后 `interval_min = null`」说的都是**人在终端里连敲**：`max_noop_streak` 不参与 runner 的停机判断。runner 那边的规矩是另一套，只有两条——
- **等得到的就等**：`user_gate` / `blocked` / `throttled` 是三种非终态，runner 一律睡下去再看（等人那两种可以用 `--exit-on-wait` 改成退出；`throttled` 等的是时间不是人，所以那个标志不管它）。
- **等不到的才退**：`paused` / `done` / `unplanned` / `all_done` / `all_deferred` / `fail_streak` / `progress_streak` / `budget`，这些不会自己好转，runner 直接停。

两边读的是同一本账，所以这条边界不划清是会串味的：`next` 记下的 `noop` 曾经能把 `throttled` 那一支的 `interval_min` 翻成 `null`，于是**人敲三下 `zloop next` 看一眼，就能让一个本来睡到窗口放开再接着跑的 runner 拒绝启动**（A-16）。

## `.zloop/` 目录与状态文件

```
.zloop/
  state.json            当前目标的唯一真源（下面的结构）
  state.json.lock       并发锁（flock）；只有写命令上锁，只读命令读 state.json 本身
  state.json.lock.holder  持锁期间才存在：谁（pid）、在干什么、什么时候拿到的；超时那句话就是读它
  goals/<id>.json       停放着的其他目标，结构和 state.json 一样（zloop goal switch 换车位）
  NOTES.md              约定（每轮都带）+ 经验（最新几条）；项目共享，不跟着目标走
  NOTES.md.bak-*        zloop reflect --apply 改写之前留的原件
  log/                  每轮一份技术文档 <时间>-<todo>-<结果>.md（思路/决策/坑/证据/改动文件）
  runner/
    journal.jsonl       runner 事件：begin / end / sleep / stop / restart / notify / commit / preflight_failed
    console.log         zloop start 的输出
    pid                 后台 runner 的 pid
  archive/              goal rm / init --force 归档的旧目标；compact 归档的旧 todo/tick
```

```jsonc
{
  "version": 1,
  "goal":   { "id": "my-project", "text": "…", "status": "active", "created_at": "…" },
  "policy": { "window_hours": 24, "max_runs": 480, "max_fail_streak": 3, "max_noop_streak": 3,
              "max_progress_streak": 8, "stale_after_min": 120, "intervals_min": [3, 10, 30],
              "max_total_usd": 0, "notify_url": "https://open.feishu.cn/…", "preflight_cmd": "cargo test -q" },
  "todos":  [ { "id": "t1", "text": "…", "priority": 0, "status": "open", "blocked_by": [],
                "note": "", "updated_at": "…", "done_at": null, "acceptance": "tests green" } ],
  "ticks":  [ { "at": "…", "round": 1, "todo": "t1", "outcome": "done", "note": "…",
                "host": "claude", "session": "11111111-…", "log": "log/20260827-055458-t1-done.md",
                "cost_usd": 0.12, "num_turns": 7, "duration_ms": 42000 } ],
  "in_progress": { "todo": "t2", "started_at": "…", "round": 2, "via": "runner", "host": "claude" },
  // compact 搬走的 tick 留下的汇总（没整理过就没有这一段）：账本变小，账不变少
  "archived": { "ticks": 12, "cost_usd": 9.5, "at": "…" },
  "next_id": 3,
  "updated_at": "…"
}
```

写入是 `tmp → fsync → rename` 原子替换；JSON 是唯一真源，`status --md` 只渲染、不回读。建议把 `.zloop/` 加进项目的 `.gitignore`——它是这台机器上的运行记录。
