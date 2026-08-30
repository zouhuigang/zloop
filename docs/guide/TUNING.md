# 调参

> `policy` 的七个字段和 runner 参数，各控制什么。
>
> ← 回到 [README](../../README.md)

**调度策略**在 `.zloop/state.json` 的 `policy`，直接编辑：

| 字段 | 默认 | 含义 |
|---|---|---|
| `intervals_min` | `[3, 10, 30]` | 有活时每 3 分钟一轮；等人/无活时 10 → 30 分钟退避；30 也是 runner 等人时的轮询周期 |
| `max_runs` | `480` | 24 小时窗口内最多记账多少轮（done / progress / fail），防空转刹车；`0` 不限 |
| `window_hours` | `24` | 上面那个窗口的长度。取值范围 `0..=8760`（一年）；写出范围的会被钳回区间，并由 `zloop doctor` 的 `bad_policy` 报出来 |
| `max_fail_streak` | `3` | 连续失败几轮停下等人 |
| `max_noop_streak` | `3` | 交互式 `next` 连续几次"没活"后停止退避。**只对 `zloop next` 生效**：`noop` tick 只有它会记，runner 一条都不记，也不拿它当停机开关 |
| `max_progress_streak` | `8` | 同一 todo 连续几轮 progress 没 done 就停；`0` 关闭 |
| `stale_after_min` | `120` | `in_progress` 多久没写回算悬挂 |
| `max_total_usd` | `0`（不限） | 本目标累计花费上限（来自 `claude -p` 返回的 `total_cost_usd`），达到即 `stopped (budget)` |
| `notify_url` | 无 | 通知 webhook。飞书自定义机器人地址会自动用飞书消息格式 |
| `notify_cmd` | 无 | 通知命令（`sh -c`），事件 JSON 从 stdin 进，另有 `ZLOOP_EVENT` / `ZLOOP_TEXT` / `ZLOOP_ROOT` 环境变量 |
| `preflight_cmd` | 无 | runner 每轮开始前先跑它（如 `./init.sh && cargo test`）；失败记一笔 `fail` 不调宿主，通过则把摘要放进 prompt。**放便宜的检查**：连红 `max_fail_streak` 轮就停机，一道跑得慢又容易红的闸会把长跑卡死在「走不到修好它那一步」 |
| `require_doc` | `true` | 完成一条 todo 必须带 `--approach`（见 [6.1](TECH-DOCS.md)）；设为 `false` 关闭强制 |
| `require_pitfall` | `true` | `--outcome fail` 必须带 `--pitfall`，失败的原因才有落点；设为 `false` 关闭强制 |

runner 起的每一个子进程都有闸——挂住的那个会被整组收掉（SIGTERM → 0.5s → SIGKILL），
runner 自己接着走，`zloop stop` 也照样叫得动。宿主那道闸是 `--timeout-min`；另外两道走环境变量：

| 环境变量 | 默认 | 管谁 |
|---|---|---|
| `ZLOOP_GIT_TIMEOUT_SECS` | `60` | zloop 自己跑的每一条 git 的闸。① `--git-commit` 每轮的 `status` / `add` / `commit` / `rev-parse`：超时按「这一轮不提交」处理，产物留在树里等下一轮认领，账本记一条 `git_stalled`；② `zloop done` 写回时列「改动文件」的 `diff` / `ls-files`（三条共用一份总预算）：超时就少写那一节，**写回照常完成**，stderr 上会说一句。仓库特别大、`git status` 要跑几十秒时调大它 |
| `ZLOOP_NOTIFY_TIMEOUT_SECS` | `30` | `notify_url`（curl）和 `notify_cmd`（`sh -c`）。通知发不出去从来不该把 runner 拖下水 |

挂住的来源不是索引锁争用（那是秒失败），是 `pre-commit` 钩子（husky / lefthook）、
`core.fsmonitor` 钩子、网络文件系统 stall。收掉 git 之后 `.git/index.lock` 万一还在，
zloop 会打一行说出来，但**不替你删**——那把锁也可能是别人正在跑的 git 拿着的。

**通知怎么配**（飞书群里加一个"自定义机器人"，拿到 webhook 地址）：

```bash
# 写进 .zloop/state.json 的 policy
"notify_url": "https://open.feishu.cn/open-apis/bot/v2/hook/xxxxxxxx"
zloop notify           # 群里收到"zloop 通知测试"即配置正确
```

之后 runner 在**等你决定**（某条 todo 被 `--block`）、**限流退避**、**停机**（除 `--max-rounds`）时各推一条，同一情形不重复。不用飞书的话 `notify_cmd` 随便接：`"notify_cmd": "osascript -e \"display notification \\\"$ZLOOP_TEXT\\\"\""`。

**runner 参数**（`start` / `run`）：`--host claude|codex`、`--timeout-min 30`、`--max-budget-usd`（仅 claude，单轮上限；总上限用 policy `max_total_usd`）、`--resume todo|all|none`、`--exit-on-wait`（等人时退出而不轮询，配合外部定时器用）、`--git-commit`（每轮写回后 `git commit`，只装这个 runner 起跑之后变化的文件，排除 `.zloop/`）、`--max-rounds N`、`--fast`（间隔按秒，演示用）、`--allow-all`。
