# 命令详解

> 每条命令干什么、什么时候敲、参数和实测例子。
>
> ← 回到 [README](../../README.md)

24 条命令，按用途分七组。**谁常敲**一列很重要：有些命令是给模型和 runner 用的，你平时不用碰。

| 命令 | 一句话 | 谁常敲 |
|---|---|---|
| **开局** | | |
| [`init`](#zloop-init-goal) | 建第一个目标 | 你 |
| [`goal`](#zloop-goal--zloop-goals) | 多目标：列出 / 新建 / 切换 / 归档 | 你 |
| [`plan`](#zloop-plan) | 写有序 todo | 你 / 模型 |
| **每一轮** | | |
| [`next`](#zloop-next) | 该不该跑、跑哪条 | 模型 / runner |
| [`done`](#zloop-done-id) | 唯一的写回口：记结果 + 留技术文档 | 模型 / runner |
| [`heartbeat`](#zloop-heartbeat) | 打印这一轮要遵守的 5 条协议 | 模型 |
| **中途调整** | | |
| [`edit`](#zloop-edit-id) | 改某条 todo 的文本 / 状态 / 优先级 / 依赖 / 验收 | 你 |
| [`pause`](#zloop-pause--zloop-resume) / [`resume`](#zloop-pause--zloop-resume) | 暂停 / 恢复整个目标 | 你 |
| [`stats`](#zloop-stats) | 这个目标跑得顺不顺：返工率、一次过、哪一步最费劲 | 你 / 模型 |
| [`reflect`](#zloop-reflect) | 回看一次：把账本 + 经验 + 反馈摆齐，整理经验（人点头才落地） | 你 / 模型 |
| [`replan`](#zloop-replan) | 重估一次：对着最终目标看剩下的任务还对不对（默认只给建议；`--apply` 才落地） | 你 / 模型 |
| [`feedback`](#zloop-feedback-todo-人说的) | 记下**你**对某一轮的回应，下一轮先处理它 | 你 |
| [`remember`](#zloop-remember-一句话) | 记一条经验（`--rule` 钉成每轮必带的约定） | 你 / 模型 |
| [`compact`](#zloop-compact) | 把老的完成项归档，state.json 保持小 | 你 |
| **看情况** | | |
| [`status`](#zloop-status) | 一屏：在哪一步 / 还剩什么 / 我该敲什么 | 你 |
| [`log`](#zloop-log) | 每轮的技术文档列表与内容 | 你 |
| [`doc`](#zloop-doc-id) | 把多轮日志合成一份完整文档 | 你 |
| [`sessions`](#zloop-sessions) | 出现过的宿主会话 + resume 命令 | 你 |
| [`context`](#zloop-context) | 有界交接包（换宿主 / 新会话先看它） | 模型 |
| [`doctor`](#zloop-doctor) | 只读体检：`.zloop` 里有没有对不上的地方，逐条给建议动作 | 你 |
| **后台长跑** | | |
| [`start`](#zloop-start--zloop-stop) / [`stop`](#zloop-start--zloop-stop) | 后台 runner 开 / 停 | 你 |
| [`run`](#zloop-run) | 前台 runner（看得见每一轮） | 你 |
| **环境接入** | | |
| [`install`](#zloop-install) | 装 skill / Stop hook / sudoers 规则 | 你（一次） |
| [`awake`](#zloop-awake-action) | macOS 睡眠保护状态与修正 | 你 |
| [`notify`](#zloop-notify-文本) | 试一下通知通道通不通 | 你 |
| **内部** | | |
| [`hook-stop`](#zloop-hook-stop) | Claude Code Stop hook 的入口，别手敲 | Claude Code |

**全局参数**：`--dir <路径>` 指定项目目录（默认从当前目录向上找最近的 `.zloop/`）；`--no-color` 关颜色（也认 `NO_COLOR`，输出不是终端时自动关）。

**退出码**：`0` 正常 · `1` 找不到 / 读不了状态文件 · `2` 参数或语义错误（未知 todo、重复 done、目标切换被拦住等）。脚本里判 `0` 就够。

**十条最常用**：

```bash
zloop init "把 CI 从 22 分钟压到 8 分钟"      # 1. 定目标
zloop plan --add "[P0] 先量出各 job 耗时"     # 2. 拆步骤
zloop start                                   # 3. 后台跑起来
zloop status                                  # 4. 看到哪了
zloop log                                     # 5. 每轮留了什么
zloop stop                                    # 6. 停
zloop goal new "另一件事"                     # 7. 换目标（旧的原地停放）
zloop goals                                   # 8. 有哪些目标
zloop edit t3 --status open                   # 9. 回答完问题，解锁那条
zloop doc --all > 技术文档.md                 # 10. 导出整个目标的文档
```

## 命令详解

### 开局

#### `zloop init "<goal>"`

**干什么**：在当前目录建 `.zloop/state.json`，写下这个项目的第一个目标。

**什么时候敲**：一个项目只需要一次。之后要换目标用 [`goal new`](#zloop-goal--zloop-goals)，不要用 `init --force`。

| 参数 | 说明 |
|---|---|
| `<GOAL>` | 目标文字。写"要达成什么"，不要写"怎么做"——怎么做是 todo 的事 |
| `--force` | 已有目标时**归档旧目标**再建新的。归档进 `.zloop/archive/`，**切不回来** |

```bash
$ zloop init "把 demo 服务的冷启动时间从 8 秒降到 1 秒以内"
initialized /path/.zloop/state.json
goal: 把 demo 服务的冷启动时间从 8 秒降到 1 秒以内
next: `zloop plan` with one `[P0] text` line per todo on stdin
```

已经有目标时直接 `init` 会拒绝（退出码 1），并告诉你当前目标是什么：

```
already initialized (done): 把 demo 服务的冷启动时间…
use --force to replace
```

目标 id 从目标文字里的英文词取（`让 keep-awake 支持外接显示器` → `keep-awake`），纯中文目标退到 `g1` / `g2`。

#### `zloop goal` / `zloop goals`

**干什么**：一个项目里管多个目标。当前目标在 `.zloop/state.json`，其余停在 `.zloop/goals/<id>.json`；切换就是把当前那份停走、把目标那份开进来。详见 [6.2](MULTI-GOAL.md)。

**什么时候敲**：手上这件事跟当前目标不是一件事的时候。**不加子命令等于 `list`。**

| 子命令 | 干什么 |
|---|---|
| `list [--json]` | 列出全部目标，`▸` 是当前那个（默认动作） |
| `new "<goal>" [--id <id>] [--force]` | 停走当前目标，开一个新的。旧的还在 list 里、可切回 |
| `switch <id\|片段> [--force]` | 切到另一个目标。认 id、id 前缀、或目标文字里的片段 |
| `rm <id\|片段>`（别名 `archive`） | 归档一个**停着的**目标：搬到 `.zloop/archive/`，从 list 消失，文件不删 |

```bash
$ zloop goal new "让 keep-awake 支持外接显示器"
停放「把 demo 服务的冷启动时间降到 …」[demo] 完成 4/4 · 切回：zloop goal switch demo
新目标 [keep-awake] 让 keep-awake 支持外接显示器

$ zloop goals
  共 2 个目标 · ▸ 是当前那个
  ▸ keep-awake  进行中   0/0  08-28 20:18  让 keep-awake 支持外接显示器
    demo        完成     4/4  08-28 15:02  把 demo 服务的冷启动时间降到 1 秒以内

$ zloop goal switch 冷启动        # 按记得的文字切，不用记 id
```

**两种情况会被拦住**（都可以 `--force` 越过，但先想清楚）：

- runner 在跑 → 换目标会让它下一轮拿着**新目标**的 todo 继续干；
- 有会话拿着 todo 还没写回 → 切走那一轮就悬在空中了。

`--force` 硬切时会当场打一行 ⚠ 说明哪一条还在别人手里；那个会话之后 `zloop done` 会被拦下并被告知先切回来，
所以成果不会记到新目标头上（要硬记：`zloop done <id> --force`）。

搬家本身是一个事务：校验（id、runner、悬空轮次）全在动文件之前，取不到锁或中途失败会把停走的那份搬回来，
不会留下"两个目标都没开着"的空档。万一真的遇到（历史遗留状态、手工搬过文件），`goal list` 照样列得出来，
按它给的 `zloop goal switch <id>` 开一个进来即可。

`rm` 只对停着的目标生效；要归档当前目标，先 `switch` 到别的。读不出来的目标（损坏 / 版本不匹配）在 list 里
显示"损坏"，也能 `rm` 掉。

#### `zloop plan`

**干什么**：往当前目标追加有序 todo。一行一条。

**什么时候敲**：定完目标之后；中途想加活也可以——给已完成的目标加 todo 会**自动把它变回进行中**。

**行格式**：`[P0] 文本 :: 验收标准`。`[P0]`/`[P1]`/`[P2]` 是优先级（数字越小越先做，可省，默认 P1）；`::` 后面是验收标准（可省），会显示在 `status` 和交给模型的 prompt 里。

| 参数 | 说明 |
|---|---|
| （无参数） | 从 stdin 读，每行一条 —— 模型规划完一批 todo 最常用这个 |
| `--add <LINE>` | 直接给一行，可重复 |
| `--file <FILE>` | 从文件读 |
| `--replace` | 先丢掉所有**未完成**的 todo 再加。注意：被 `--block` 卡着的那些也会一起没了；已完成 / 已延后的留着 |
| `--from-loopx <ACTIVE_GOAL_STATE.md>` | 从 loopx 的状态文件导入未完成的勾选项 |

```bash
$ zloop plan <<'EOF'
[P0] 量出各 job 的耗时，找出最慢的三个 :: 有一张耗时表
[P0] 把测试拆成三个并行 job :: CI 总时长 < 12 分钟
[P1] 缓存 cargo registry
EOF
t1 [P0] 量出各 job 的耗时，找出最慢的三个 :: 有一张耗时表
t2 [P0] 把测试拆成三个并行 job :: CI 总时长 < 12 分钟
t3 [P1] 缓存 cargo registry
```

**一条建议**：todo 要"一轮能做完、能验证"。太大的 todo 会连续几轮"有进展"却完不成，第 8 轮调度器会停下来让你拆小。

### 每一轮

#### `zloop next`

**干什么**：回答两个问题——**现在该不该跑**，以及**跑哪一条 todo**。这是整个调度器的入口。

**什么时候敲**：模型每轮开头敲，runner 每轮开头也敲。你自己一般不需要，想看"下一轮会做什么"用 `--peek`。

它做三件事：按 [`next` 决策梯](INTERNALS.md)算出 `should_run`；把选中的 todo **交出去**（`phase` 变成"执行中"，写 `in_progress`）；没活可干时记一笔 `noop`（用于退避计数）。

| 参数 | 说明 |
|---|---|
| `--json` | 机器可读：`should_run` / `reason` / `todo` / `interval_min` / `remaining` / `writeback` / `phase` 等 |
| `--peek` | 只看不交出，也不记 noop —— 你想"瞄一眼"就加这个 |

```bash
$ zloop next
RUN  t2 [P0] 把测试拆成三个并行 job
     acceptance: CI 总时长 < 12 分钟
     writeback: zloop done t2 --note '<一句话结果>'
     interval: 3 min · remaining 477
     phase: executing t2 · round 5 · since 20:41 (0s ago) · host claude · via next

$ zloop next            # 没活干的时候
WAIT (user_gate) remaining 477 · retry in 10 min
```

#### `zloop done <id>`

**干什么**：**唯一的写回口**。记一笔 tick、改 todo 状态、写这一轮的技术文档，必要时插入后继 todo。

**什么时候敲**：每轮结束时，做了什么就写什么。别的命令都不写执行历史。

| 参数 | 说明 |
|---|---|
| `--note <一句话>` | 结果摘要，会出现在 `status`、`context`、日志头部 |
| `--outcome done\|progress\|fail` | 默认 `done`。`progress` = 有进展但没完（todo 留着）；`fail` = 这轮失败（连续 3 次会停下来） |
| `--block <问题>` | 卡在你身上：todo 标成"等你回话"，问题原文印在 `status` 里 |
| `--next <LINE>` | 顺手插一条后继 todo，排在这条后面 |
| `--approach <文本\|@文件>` | **实现思路**：怎么做的、为什么这么做。`outcome=done` 时必填（见 [6.1](TECH-DOCS.md)） |
| `--decision <文本>` | 关键决策 / 取舍，可重复 |
| `--pitfall <文本>` | 遇到的坑与结论，可重复 |
| `--evidence <文本\|@文件>` | 验证证据：命令输出、测试名、测量值 |
| `--no-doc` | 这一轮不写技术文档（绕过 `policy.require_doc` 和 `policy.require_pitfall`） |
| `--force` | 派活来自别的目标时也照记到当前目标。默认会拦下来，让你先 `zloop goal switch <原目标>`（见 [6.2](MULTI-GOAL.md)） |

```bash
$ zloop done t2 --note "CI 从 22 分钟降到 9 分 40 秒" \
    --approach "拆成 lint / unit / e2e 三个 job 并行；e2e 用 matrix 再切两片" \
    --decision "不引入 nx 之类的缓存工具，先把并行度吃满再说" \
    --pitfall "并行后 flaky 测试暴增，根因是三个 job 共用同一个测试数据库" \
    --evidence "最近 5 次 main 构建：9m40s / 9m12s / 10m01s / 9m33s / 9m48s"
t2 done: CI 从 22 分钟降到 9 分 40 秒
remaining 1 · next: t3 [P1] 缓存 cargo registry
log: .zloop/log/20260828-204107-t2-done.md
```

**注意**：`--approach` / `--evidence` 支持 `@文件`。带反引号或引号的长文本，**用 `@文件` 或单引号**——双引号里的反引号会被 shell 当命令执行。

#### `zloop heartbeat`

**干什么**：打印"这一轮要遵守的 5 条协议"，给模型看。它自己不改任何状态。

**什么时候敲**：`/zloop` 无参数时，skill 第一件事就是敲它。

| 参数 | 说明 |
|---|---|
| `--host claude\|codex-app\|codex-cli` | 按宿主调整措辞（默认 `claude`） |

协议就是那五步：`zloop context` → `zloop next --json` → **只做那一条 todo** → `zloop done <id> …` → 两三句话汇报。

### 中途调整

#### `zloop edit <id>`

**干什么**：改某一条 todo。**任何 `edit` 都算"人介入"，会重置连续失败 / 连续空转计数**——所以调度器停下来之后，改完就能接着跑。

**什么时候敲**：回答了被 `--block` 的问题、要拆小某条 todo、要调顺序、要挂依赖的时候。

| 参数 | 说明 |
|---|---|
| `--text <文本>` | 改文字（拆小 todo 最常用） |
| `--status open\|blocked\|deferred\|done` | 改状态。`open` 是"解锁"，`deferred` 是"先放着别做" |
| `--priority <0-4>` | 改优先级，影响执行顺序 |
| `--blocked-by <IDS>` | 逗号分隔的 todo id，或 `user`（等你回话）；`''` 清空 |
| `--acceptance <文本>` | 改验收标准；`''` 清空 |

```bash
$ zloop edit t3 --status open              # 回答完问题，放它回队列
$ zloop edit t3 --text "只做配置懒加载，压测拆成 t5"   # 太大，拆小
$ zloop edit t5 --blocked-by t3            # t5 得等 t3 做完
```

回显里的依赖是 `⏳t3`（在排队）还是 `⛔等不到 t3`（永远轮不到，判据同 `zloop status`
进展列里的那两个字），后者下面跟着解开的命令：

```bash
$ zloop edit t5 --blocked-by t3            # t3 已经被 --status deferred 挂起了
t5 [P1] open 压测 ⛔等不到 t3
  ↳ 解开敲 zloop edit t3 --status open
```

`--blocked-by` 只挡自依赖和不存在的 id，依赖一条已延后的 todo 是放行的——所以这句话
必须在**造出它的那一刻**说，不然下次知道是 `doctor` 退 1 的时候。同一句话也印在
`zloop context`（模型每轮读的交接包）和 `zloop status --md` 里。

#### `zloop pause` / `zloop resume`

**干什么**：把整个目标按住 / 放开。`pause` 之后 `next` 一律说"停"，后台 runner 在下一次检查时自己退出；todo 一条不动。

**什么时候敲**：临时要用机器干别的、或者想让它先别动的时候。比 `zloop stop` 更"硬"——`stop` 只是停掉 runner，你 `/zloop` 还能跑一轮；`pause` 是连人工那一轮也不给跑。

```bash
$ zloop pause
goal is now paused
$ zloop resume
goal is now active
```

#### `zloop stats`

**干什么**：把账本里已经记着的东西汇成"这个目标跑得**怎么样**"。和 `status` 分工明确——
`status` 答"还剩什么、我该敲什么"，`stats` 答"顺不顺、哪一步最费劲"。

**什么时候敲**：目标跑了十几轮之后想知道钱花在哪、哪条反复返工；或者在决定"要不要换个做法"之前。

```
  统计    有返工有失败的目标

  轮次    5 轮 · 返工 3（60%）· 失败 1
  质量    一次过 1/2 条 · 无文档 1 轮 · 被挡 1 次 · 用户反馈 1 条
  最费劲  t2 返工 2 次

  ┌──────┬────┬──────────────┬──────┬──────┬──────┬──────────┐
  │ 步骤 │ id │ 这一步做什么 │ 轮次 │ 返工 │ 文档 │ 结果     │
  ├──────┼────┼──────────────┼──────┼──────┼──────┼──────────┤
  │    1 │ t1 │ 顺利的一条   │    1 │    — │ 有   │ 一次过   │
  │    2 │ t2 │ 反复改的一条 │    3 │    2 │ 缺   │ 完成     │
  │    3 │ t3 │ 会失败的一条 │    1 │    1 │ —    │ 在做     │
  │    4 │ t4 │ 要问人的一条 │    — │    — │ —    │ 等你回话 │
  └──────┴────┴──────────────┴──────┴──────┴──────┴──────────┘
```

| 数字 | 怎么算的 |
|---|---|
| 轮次 | `done` + `progress` + `fail` 的 tick 数（`block` / `noop` / `edit` / `feedback` / `reflect` / `replan` 都不算）；`zloop status` 标题上的「跑了 N 轮」用的是同一个定义。被 [`compact`](#zloop-compact) 归档走的轮次**也算**——这是目标一辈子的数，整理账本不会让它掉下去 |
| 返工 | `progress` + `fail` 的轮数；括号里是它占轮次的比例 |
| 一次过 | 一轮做完、中间没返工过的 todo 数 ÷ 已完成的 todo 数 |
| 无文档 | `documented == false` 的轮次（`zloop log` 里带 ⚠ 的那些） |
| 最费劲 | 返工最多的那条，其次看失败、被挡 |
| 花费 | 只在宿主报过 `cost_usd` 时才出现（交互式轮次没有这个数） |

`--json` 给脚本用，字段和上表一一对应，另有每条 todo 的明细。

**为什么会有这个命令**：Warp 的自改进回路是 **跑 → 打分 → 自改进**，`RunScorer` 就在自改进的前一环
（见 [`docs/SELF-IMPROVEMENT.md`](../../docs/design/SELF-IMPROVEMENT.md)）。zloop 此前只有"有没有留实现思路"这一个布尔值，
`stats` 是把打分这一环补上——它同时是 reflect 的输入。

#### `zloop replan`

**干什么**：对着**最终目标**重估剩下的任务——还能做成吗？漏了什么？哪条已经没意义了？
把目标、刚做完那轮、剩余 todo（连验收标准）、触发的信号、**你说过的原话**摆成一页给模型，
要它提**最小改动**。命令本身只读，一个字都不改。

**什么时候敲**：`zloop done` 提示你的时候（见下），或者你自己觉得计划偏了。

**它不会天天烦你**：每次 `done` 之后 zloop 会做一次**纯代码的体检**，读得出偏离信号才提一句：

```
⚠ 计划可能要调整：t2 有你的反馈（已经过了一轮） · t2 连续 4 轮没做完 · 返工率 80%（最费劲的是 t2）
  想清楚剩下的任务还对不对：zloop replan
```

没命中就**一个字都不说**。六个信号全部读自账本：

| 信号 | 什么时候亮 | 它在问 |
|---|---|---|
| `feedback` | 还没做完的 todo 上出现过你的反馈 | 出岔子了吗 |
| `stalled` | 同一条连着 ≥2 轮没做完 | 出岔子了吗 |
| `fail_streak` | 连续 ≥2 轮失败 | 出岔子了吗 |
| `rework` | 返工率 ≥50%（且已跑 ≥3 轮） | 出岔子了吗 |
| `blocked` | 有 todo 在等你回话 | 出岔子了吗 |
| **`rethink`** | 某一轮写回时带了 `--rethink` | **还到得了目标吗** |

**`rethink` 是唯一不问"出岔子"的那个。** 最该重规划的场景恰恰不偏离：那一轮**顺利完成**，
可它的结论把剩下几条的前提推翻了——没失败、没停滞、没返工、没被挡，前五个信号一个都不响。

zloop 读不出"策略走不通"（不做关键词嗅探：既不可靠又只认一种语言），所以只认干活的人主动说的那一句：

```bash
zloop done t2 --note "加了缓存只省 30ms" --approach "LRU" \
  --rethink "瓶颈根本不在读取，在反序列化——后面三条全建立在「缓存有效」这个前提上，前提没了"
```

和邻居的区别：`--pitfall` 是"这条路上有个石头"，`--rethink` 是"这条路本身不通往目标"，
`--block` 是"我需要人来回话"。命中之后材料包会把这句原话完整摆给模型，并允许它**照新现状重排**——
其余情况一律还是"别重开一张清单"。

**为什么不是每轮都重估**：文献（[Bayesian partner modelling](https://arxiv.org/html/2608.18490)）明确说选择性触发能用
**远少于**启发式/LLM 触发的重规划次数拿到相当收益；每轮调模型重估不但贵，还会**制造计划抖动**——
模型每被问一次"要不要改"就有概率改一点。所以：**沉默是默认**。完整依据见
[`docs/ADAPTIVE-REPLAN.md`](../../docs/design/ADAPTIVE-REPLAN.md)。

**改不改你点头**：`replan` 只给建议，落地走现成的 `zloop plan --add` / `zloop edit`。
提示词里还专门写了一句「**不用改是完全合格的结论**」，防止为了改而改。

**无头也有**：`zloop start` 默认开着这个——写回之后如果信号命中，runner 会插一轮重估，
**只把建议记进账本**（`zloop log` 里看得到），**绝不自己动 todo**。`--no-replan` 关掉。

---

##### 让它自己改：`zloop replan --apply` 与 `--auto-replan`

上面那套只提议不落地。想让循环**自己换路线**——做到第 2 步发现整条路线的前提没了，
就重排剩下的路继续跑——两步：

```bash
# 1) 落地通道：从 stdin 收新清单（只列还没做的，做完的和等你回话的自动留着）
printf '%s\n' '[P0] 量反序列化耗时 :: 有逐字段表' '[P0] 换零拷贝路径 :: 快 300ms' \
  | zloop replan --apply --why "实测瓶颈在反序列化，加缓存整条路线作废"
# → replan applied: 换掉 3 条、新排 2 条、保留 2 条（已完成和等你回话的没动）
#   旧账本备份在 .zloop/state.json.bak-...

# 2) 无头自主（默认关）
zloop start --auto-replan
```

**六条护栏在代码里强制**，不是写在提示词里——违反就**整体拒绝**并指名是哪条
（半途改一半的计划比不改更糟）：

| 护栏 | 不加会怎样 |
|---|---|
| 清单不能空 | 重排成 0 条 = 悄悄放弃目标 |
| 每条都要带 `:: 验收` | 说不出怎么验，就是没想清楚它凭什么算一步 |
| `--why` 必填 | 事后没人看得出这次改动想解决什么 |
| 规模 ≤ 3 倍 + 5，且总数 ≤ 30 | 一次炸出两百条 todo，跑到天荒地老 |
| 有轮次在飞就不改 | 那个 agent 手上拿的 todo 可能正要被换掉 |
| 不动「已完成」和「等你回话」的 | 前者动了等于抹历史；后者身上挂着一个**给你的问题** |

新 id 从 `next_id` 往后发、**不复用**（复用会让老 tick 挂到新 todo 上，账本对不上）；
改前 `state.json` 自动备份。

**无头模式下默认不许改计划，这是代码闸不是提示词**：runner 只在 `--auto-replan`
且正是重估那一轮时，才给子进程放行 `ZLOOP_AUTO_REPLAN`；`replan --apply` 见到
`ZLOOP_RUNNER` 而没有它就拒绝。干活轮次、回看轮次、`preflight` 一律不放行。

**跑飞了会停在你面前**，不是安静地接着跑。两条闸任一触顶就停机（`stop reason=replan_diverged`）：

- 单次运行最多自主改 **3** 次
- **连着两次都把清单改长** = 在发散不是在收敛

```
runner: 计划改了 · 1 条 → 4 条（第 1/3 次自主重排）
runner: 计划改了 · 3 条 → 5 条（第 2/3 次自主重排）
runner: 停下来等人 —— 连着 2 次重排都把清单改长了（这次 3 → 5）——在发散，不是在收敛
```

计划到底动没动**不听宿主自称**：改完重读账本比对 todo id，动了才计数、才记
journal 的 `replan_applied`。完整设计见
`docs/ADAPTIVE-REPLAN.md` 的 [§6 三个缺口](../../docs/design/ADAPTIVE-REPLAN.md#6-三个缺口各有代码为证)–[§10 落地](../../docs/design/ADAPTIVE-REPLAN.md#10-落地三条-todo-各管一段)。

#### `zloop reflect`

**干什么**：不做 todo 的那一轮。把**全部**经验、失败与坑、用户说过的话、`stats` 的几个数字摆在一页上，
外加几项机械体检（约定攒得太多了没有、哪两条经验像是同一件事、有几条已经被交接包的窗口挡在外面），
交给模型判断该**保留 / 合并 / 删掉**什么。

**什么时候敲**：跑了十几轮之后；或者你发现 `zloop context` 里的经验开始重复、开始过时的时候。

```
$ zloop reflect
# 回看一次：把冷启动降到 1 秒

跑了 12 轮 · 返工 4（33%）· 失败 2 · 被挡 1 次 · 无文档 0 轮 · 用户反馈 3 条
最费劲的是 t3：返工 3 次

## 现有约定（`.zloop/NOTES.md`，**每轮都带给模型**）
R1. done 之前一定要跑 cargo test
## 现有经验（全部 9 条，但每轮只带最新 5 条）
1. [08-27] bench.sh 要在 release 模式下跑（窗口外，模型看不到）
2. [08-29] bench 脚本必须用 release 模式跑，debug 差 3 倍
…
## 我当时怎么说 vs 你怎么回的（**要改进的就是这个差**）
## t1 · 2026-08-29T07:35
- 我当时说：用正则实现了（实现思路：正则最快，输入看着很规整）
- 你回的：正则不行，输入会有嵌套括号，换成手写状态机
## 机械体检（代码能看出来的）
- 第 1 条和第 2 条像是同一件事，考虑合并
- 共 9 条经验，但 `zloop context` 每轮只带最新 5 条——前 4 条模型永远看不到，该合并或删掉
## 你要做的
1. 逐条判断：升格成**约定**（每轮都带）、留作**经验**（会轮换）、合并，还是删掉…
2. 讲给用户听…  3. **人点头之后**才落地…
```

那一节「我当时怎么说 vs 你怎么回的」是整份材料里信息量最大的：`zloop feedback` 记下的每句话，
都会**配到它回应的那一轮**上，和当时的一句话结果 + 实现思路摘要并排。没人回过话的轮次不占版面。
Warp 的 improver 读的正是这个差——两栏各列一遍是看不出差的。

**约定这一层也有体检**：经验有窗口兜底（写多了老的自己滚出去），约定**不轮换**——写多少条就每轮
全量占多少篇幅，挤掉的是交接包尾部那几节。所以攒到第 11 条时，体检里会多出这么一行：

```
- 共 11 条约定，超过 10 条——约定不轮换，每轮全量进交接包（约 233 字，占默认预算 5%），挑几条降回经验或删掉
```

条数听着不吓人，**占默认预算百分之几**才是代价，所以两个数一起给。阈值默认 10，`--max-rules N` 可调
（`zloop reflect --max-rules 15`）——调高就是明确表态"我这个项目就是规矩多"，而不是把提示当噪音忍着。

**zloop 自己不下判断**：它只摆材料、做机械体检。判断是模型的事，**落地要人点头**：

```bash
$ zloop reflect --apply <<'EOF'
## 约定
- done 之前一定要跑 cargo test
## 经验
- bench 脚本必须用 release 模式跑，debug 差 3 倍
EOF
约定 0 → 1 条 · 经验 9 → 1 条：/path/.zloop/NOTES.md
  旧的备份在 /path/.zloop/NOTES.md.bak-20260829T070655+0800
```

不写小标题就全按经验处理（老用法不变）。

改之前一定先备份（这是 zloop 里唯一一个会删掉你写的东西的操作）。stdin 里的编号（`1. `）和短横线会被容忍，
空输入直接拒绝——不会把"什么都没说"当成"全删掉"。

**无头也能定期回看**：`zloop start --reflect-every 5` 让 runner 每 5 个 todo 轮次插一轮回看。
那一轮**不做 todo、不推进轮次编号、对三条 streak 透明**（插一轮反思不代表失败被解决了），
而且**不会自己改 NOTES.md**——无头模式没人点头，所以它只把建议记进账本（`zloop log` 里看得到），等你回来看。

**跑起来长什么样**：下面这段是在一个玩具项目（3 条 todo：写 `hello.py`、加 `--name`、写 README）上真跑的一次，
原样贴过来。`--fast` 把轮间隔从分钟压成秒，所以 `--timeout-min 240` 在这里是 240 秒：

```console
$ zloop run --host claude --fast --timeout-min 240 --reflect-every 2 --max-rounds 3 --no-keep-awake --no-color
runner: round 1 → t1 [claude]
runner: round 1 written back · 本轮完成 t1：新建了 `hello.py`（模块 docstring + `main()` + `__main__` 守卫，首行带 shebang），无参数运行精确输出 `Hello, world!`，退出码 0。
runner: session → claude --resume 6fe679d6-3956-48f8-a951-a7ee614e653a
runner: round 2 → t2 [claude]
runner: round 2 written back · 本轮做了 t2：用 argparse 给 `hello.py` 加了 `--name` 参数，default 设成 `world`，输出统一走 `f"Hello, {args.name}!"` 一条分支。
runner: session → claude --resume 1065ab08-d106-475d-822c-855e9777bc2b
runner: 第 2 轮之后插一轮回看（不占轮次）                    ← 回看那一轮，就这两行
runner: 回看写进账本 · log/20260829-095120-reflect.md        ←
runner: round 3 → t3 [claude]
runner: round 3 written back · 本轮完成 t3：写了 `README.md`——一句话说明 + 两条用法示例（`python3 hello.py` → `Hello, world!`，`python3 hello.py --name Ada` → `Hello, Ada!
runner: session → claude --resume 3b73637c-f642-444b-a5a9-82cadaac493c
runner: max rounds reached
runner: stop (max_rounds)
```

**要看的就是那两行前后**：回看没有 `round N → tN` 那一行（它不领 todo），而 round 3 直接接着 round 2 往下数
——轮次编号没被它推进。`.zloop/runner/journal.jsonl` 里也只多一条不带轮次的事件：

```json
{"event":"reflect","after_round":2,"at":"2026-08-29T09:49:20+08:00"}
```

账本里它是一条**不挂在任何 todo 上**的 `reflect` tick，`stats` 单独给它一个计数，三条 todo 该做完的照样做完
（`轮次` 只数 `done`/`progress`/`fail`，回看不在其中；`zloop status` 标题上的「跑了 N 轮」和这个数一致）：

```console
$ zloop stats
  轮次    3 轮 · 返工 0（0%）· 失败 0
  质量    一次过 3/3 条 · 无文档 0 轮 · 被挡 0 次 · 用户反馈 0 条 · 回看 1 次
  花费    $1.90 · 宿主累计 4m
```

建议本身落在 `zloop log` 列出的那份 `-reflect.md` 里，**全文**，等你回来看：

```console
$ zloop log
  .zloop/log/20260829-095223-t3-done.md  t3 · done · 2026-08-29T09:52:23+08:00
  .zloop/log/20260829-095120-reflect.md  回看 · 第 2 轮之后
  .zloop/log/20260829-094909-t2-done.md  t2 · done · 2026-08-29T09:49:09+08:00
  .zloop/log/20260829-094826-t1-done.md  t1 · done · 2026-08-29T09:48:26+08:00

$ zloop log --show 20260829-095120-reflect.md
# 回看 · 第 2 轮之后

看完了两轮的全部账本（`state.json`、两份 round log、runner journal、`hello.py`、console log）。现状：2 轮全 done、全 documented、0 返工 0 失败 0 反馈，`NOTES.md` 还不存在，约定和经验都是 0 条。

顺带一个事实先说清楚：**仓库到现在一个 commit 都没有**（`git log` 报 no commits yet），`hello.py` / `.zloop/` / `runner-console.log` 全是未跟踪文件。

## 建议清单

## 升格成约定（1 条）

**A. `done` 之前必须实跑验证并把证据写进日志：新行为 + 前面轮次已交付行为的回归，贴精确输出串和退出码；跑不通就不写 done。**

（…中间 40 行略：4 条留作经验的、3 条主动丢掉的，各自附了为什么…）

## 落地载荷（本轮不执行）

这一轮是 runner 无头驱动、没人点头，所以我没有运行 `zloop reflect --apply`，也没动任何代码和 todo。等你回来认可后，把下面这段从 stdin 交给它即可：

```
## 约定
- done 之前必须实跑验证并把证据写进日志：新行为 + 前面轮次已交付行为的回归，贴精确输出串和退出码；跑不通就不写 done。
...
```
```

**只有"中间 40 行略"那一句是我加的**，其余逐字来自那份 55 行、4556 字节的文件——**宿主说了什么就存什么，不截断**。
回看不写回账本，这份全文是它唯一的产物（`tick.note` 上那 200 字只是账本里的一句摘要）；干活轮次不走这条路，
它们的技术文档是 agent 自己用 `zloop done --approach` 写的。

> 早先的版本在 300 字处截断，这份文件只剩个开头——建议清单的后半截连同「落地载荷」一起丢了。
> 现在全文落盘，`tests/runner_test.rs` 里那条回看测试用一段超过 300 字的宿主输出盯着它。

这套形状是照 Warp 抄的：他们的 improver 是**按计划跑的观察者**，数据模型只有 cron + prompt + enabled +
last_spawn_error 六个字段——反思不需要新子系统，它就是"隔一阵子换一段 prompt 跑一轮"
（见 [`docs/SELF-IMPROVEMENT.md`](../../docs/design/SELF-IMPROVEMENT.md)）。

#### `zloop feedback <todo> "<人说的>"`

**干什么**：把**你**对某一轮的回应记进账本。`note` / `approach` / `pitfall` 全是模型自述，
`feedback` 是唯一一个人写的——有了它才算得出"模型建议的"和"你接受的"之间的差。

**什么时候敲**：模型交了一轮但方向不对、或者你想补一句它不知道的前提时。别只在对话里说，说完就没了。

```bash
$ zloop feedback t1 "正则不行，输入会有嵌套括号，换成手写状态机"
feedback → t1：正则不行，输入会有嵌套括号，换成手写状态机
下一轮的 `zloop context` 会带上
（t1 已经是 done；要让它重做：`zloop edit t1 --status open`）
```

记下之后：

- `zloop context` 多出一节 **「用户对上一轮的反馈（先处理这些）」**，排在「下一条」前面，永远不会因为篇幅被裁掉；
  每轮协议也明说了"有反馈就先按它调整"。
- `zloop status` 多一行 `反馈`，你自己也看得见（免得"我说了它没反应"无从判断）。
- `zloop doc <todo>` 里，反馈紧跟在它回应的那一轮后面，和模型的实现思路并排——事后翻文档能看出方向为什么变。
- **`noop` streak 会被它打断**：循环停下来等人，人开口说话正是它该等到的东西。
  实测连续 3 次 `fail` 之后 `next` 是 `WAIT (fail_streak)`，`zloop feedback …` 之后立刻变回 `RUN`。
- `fail` / `progress` 那两条**停机**闸要多一个条件：**只有循环已经停在那条 streak 上**
  （失败数够到 `max_fail_streak` / 同一条 todo 的 progress 够到 `max_progress_streak`），
  这句反馈才清零。还在跑的时候补一句「先别动 x.rs」不算"失败被解决了"、也不算"这条活不再
  原地踏步了"——无条件清零等于给无头 runner 拆保险丝：反馈一插进两次 fail（或两轮 progress）
  中间，计数就永远数不到上限，宿主一轮一轮接着烧（A-17 / A-21）。
- 不吃配额、不推进轮次（`feedback` 不在计数的 outcome 里）；不改 todo 状态、不碰在飞状态。要让一条已完成的
  todo 重做，照旧是 `zloop edit <id> --status open`。

反馈跟着**目标**走（存在 `state.json` 里），所以多目标之间不会串。
处理过的反馈（后面又有 `done` / `progress` 轮次）不再占交接包版面，但一直留在 `zloop doc` 里。

#### `zloop remember "<一句话>"`

**干什么**：往 `.zloop/NOTES.md` 记一条经验。最新几条会自动出现在 `zloop context` 里。

**什么时候敲**：发现一个模型总是踩的坑、或者一条项目特有的约定时。这是纠正长程任务里反复犯的错最省力的办法。

```bash
$ zloop remember "fmt check 在 CI 上偶发失败，重跑即可，不要改代码"
remembered → /path/.zloop/NOTES.md

$ zloop remember --rule "done 之前一定要跑 cargo test"
约定 +1（共 1 条，每轮都带给模型）→ /path/.zloop/NOTES.md
```

经验是**项目共享**的，不跟着目标走。

##### 两层：约定 vs 经验

`.zloop/NOTES.md` 分两段，区别只有一个——**会不会轮换**：

```markdown
## 约定（每轮都带）          ← 全量注入交接包，多少条都带
- done 之前一定要跑 cargo test

## 经验（最近 5 条会带）      ← 只带最新 5 条，写多了老的就看不到了
- 2026-08-29T07:00:00+08:00 bench.sh 要在 release 模式下跑
```

`remember` 写的是**经验**。要钉一条**约定**，两条路：

```bash
zloop remember --rule "done 之前一定要跑 cargo test"   # 你顺手钉一条，立刻生效
zloop reflect                                          # 让模型看完材料再建议升格哪几条，你点头后 --apply
```

`--rule` 是**给你用的**：约定每轮都注入、不轮换，等于给这个项目加了一条硬规矩。
每轮协议里没有教模型用它——模型该走 `reflect` 那条路（建议 → 你点头），这样才留得住人在环里。
重复钉同一条会被识别出来，不会重复。

**为什么需要这一层**：Warp 的自改进回路里，改进落在 base skill 上——那是下一轮一定会读的东西。
zloop 的 `SKILL.md` 却是**全局**的（`~/.claude/skills/zloop/`），把某个项目的规矩写进去会污染别的项目；
而经验只带最新 5 条，写到第 20 条时前 15 条对模型等于不存在。所以真正缺的不是"写进 skill"，
而是一个**项目级、每轮必读、不轮换**的位置——就是「约定」。约定要少：它每轮都占交接包的篇幅。

老格式（没有小标题的一串 `- `）照旧能读，全部按经验处理。

##### 边界：经验和约定都不跨项目（这是取舍，不是缺陷）

`.zloop/NOTES.md` 是**项目级**的：在 A 项目 `remember` 下来的东西，到了 B 项目一个字都看不到。
`remember` 没有 `--global`，`reflect` 也只读当前项目这一份。为什么这么定：

- **绝大多数经验本来就长在项目上**——"bench.sh 要在 release 模式下跑，debug 数据差 3 倍"只对这个仓库成立，
  搬到别处是噪声。
- **写错一条的代价不对等**：一条错的项目约定只坑一个目录；一条错的全局约定，**每个项目每轮**都要为它付
  交接包的篇幅，还都被它误导一次，而且没人会想起来回去删。
- **重抄一遍很便宜**：到新项目再敲一次 `zloop remember` 是一行命令；判断"这条到底普不普适"要贵得多，
  而做这个判断最好的时机是你**第二次敲同一句话**的时候，不是第一次。

**今天已经有一条手动路径**：[`zloop install`](#zloop-install) 写的 `~/.claude/skills/zloop/SKILL.md` 是**全局**的，
它的用户区（`<!-- zloop:user -->` 之后）`install` 永远保留——真正跨项目成立的话写在那里，所有项目的 `/zloop` 都读得到。
没有任何命令会写它，是有意的：全局的东西该由人手动落笔。

##### 如果要做：最小形态（没实现，[#9](https://github.com/zouhuigang/zloop/issues/9)）

一句话：**再开一份同格式的全局 NOTES，只做「约定」那一层。**

| 决定 | 怎么定 | 为什么 |
|---|---|---|
| 存哪 | `~/.zloop/NOTES.md` | 全局状态都收在 `~/.zloop/`（`awake/` 已经在那儿，[跨项目视图](#如果要做最小形态没实现9)那份设计也落在这里）；解析仍走同一份 `notes.rs`，不引入第二种格式 |
| 带哪层 | **只带约定，不带经验** | 经验只有最新 5 条的窗口；全局经验会跟项目经验抢这 5 条，而且抢赢了也讲不出道理 |
| 怎么写 | `zloop remember --global --rule "<一句话>"` | 多一个 flag，不多一条命令 |
| 怎么注入 | `context` 里排在项目约定**前面**，行首标「全局」 | 模型要能一眼分清"这条到处都成立"和"这条只在这个仓库成立" |
| 怎么管 | `reflect` 的材料里把全局约定单列一段，可以建议"升到全局 / 从全局降回项目"，`--apply` 按小标题写回各自的文件 | 保持"模型建议 → 人点头"这条唯一的落地路径 |
| 上限 | 全局比项目更严：项目 10 条，全局给 5 条；[reflect 的第三项体检](#zloop-reflect)把两边加起来一起算预算占比 | 全局约定是**每个项目每轮**都在付的钱 |

改动量估计：`notes.rs` 把路径参数化 + 一个 `global_path()`、`cli.rs` 一个 flag、`context.rs` 多注入一段、
`reflect.rs` 的体检多算一份——约 100 行代码 + 3 个测试（写得进全局 / 注入时两段都在且分得清 / 全局超限时体检出声）。

**什么时候才做**：等你在第二、第三个项目里发现自己在重敲同一条 `remember`。那时候需求是真的，
手上也正好有"它在几个项目里都成立"的证据。在那之前，多这一层只是多一个会过时的地方。

#### `zloop compact`

**干什么**：把很久以前完成 / 延后的 todo 和它们的 tick 搬进 `.zloop/archive/`，让 `state.json` 保持小、让 `status` 保持短。

**什么时候敲**：目标跑了几十轮、`status` 的清单开始翻页的时候。

| 参数 | 说明 |
|---|---|
| `--keep-days <N>` | 完成在 N 天内的留着（默认 7） |
| `--force` | runner 在跑 / 有轮次没写回时照常整理 |

技术文档（`.zloop/log/*.md`）不会被搬走，永远留在原地。

**还有人等的那条不搬**：一条做完的 todo，如果还有没做完的 todo 的 `blocked_by` 指着它，
`compact` 会把它留在清单里（其余的照常整理），并印一行说明：

```
compacted 1 todos and 1 ticks → .zloop/archive/compact-….json
  ⏸ 留下 1 条没搬：还有没做完的 todo 在等它们
     t1 ← t2,t3
  ↳ 搬进归档就再也捡不回来；等它们做完，或 zloop edit t2 --blocked-by ''
```

搬走它等于把 t2 / t3 判死刑（`doctor` 会退 1 报 `dangling_blocked_by`），而**归档里的
todo 没有命令能捡回来**，唯一的出路是把 t2 的依赖断开，连"它当初依赖谁"也一起丢。
留下不是永久钉住：等的人一做完或一了结，下一次 `compact` 自然会带上它。

**账不跟着 tick 走**：搬走的 tick 会在 `state.json` 的 `archived` 里留下一份汇总——花费
（`cost_usd`）、按结果分的轮数（`outcomes`）、无文档轮数、宿主耗时。所以整理**不会**让
这些数掉下去：`policy.max_total_usd`（这个目标一共只准花这么多）照旧按累计算，
`zloop status` 的「跑了 N 轮」、`zloop stats` 的轮次/返工率、`zloop replan` 的返工信号
也都还是这个目标**一辈子**的数——整理账本不是重新开始。带走了钱它会在输出里说一声，
`zloop stats` 会多印一行「归档」说明清单为什么比轮数短。

只有**从 todo 数出来**的两个数会跟着清单缩短：`status` 的进度条百分比，和 `stats` 的
「一次过 X/Y 条」。整理走的 todo 连 id 都不在了，这两处只讲还在清单里的那些。

同理，`compact` 改的是 runner 下一轮要读的轮次记录，
所以和 `zloop goal switch` 一样：runner 在跑、或有轮次没写回的时候会拒绝，`--force` 放行。

### 看情况

#### `zloop status`

**干什么**：一屏回答三个问题——**现在在哪一步 / 还剩什么 / 我该敲什么**。详见 [6](../../README.md#6-看进度status--log--sessions--context)。

**什么时候敲**：任何时候。它是只读的。

| 参数 | 说明 |
|---|---|
| `--json` | 整份状态（脚本用这个，别 grep 人类视图） |
| `--md` | Markdown 投影：每条 tick 带 resume 命令和日志链接，可重定向成文件给人看 |
| `--no-color` | 纯文本（管道 / 重定向时自动就是纯文本） |

#### `zloop log`

**干什么**：列出或打开每一轮的技术文档（`.zloop/log/<时间>-<todo>-<结果>.md`）。缺实现思路的轮次会打 `⚠`。

**什么时候敲**：想知道"上一轮到底干了什么、为什么这么干"，或者调度器因连续失败停下来要看原因的时候。

| 参数 | 说明 |
|---|---|
| `--last <N>` | 最近 N 条，时间倒序（默认 20） |
| `--todo <id>` | 只看某条 todo 的每一轮 |
| `--show <FILE>` | 打印某一份日志（可以给全路径，也可以只给文件名） |

```
$ zloop log --todo t2
  .zloop/log/20260828-204107-t2-done.md  t2 · done · 2026-08-28T20:41:07+08:00
  .zloop/log/20260828-193355-t2-progress.md  t2 · progress · 2026-08-28T19:33:55+08:00

$ zloop log --show 20260828-204107-t2-done.md     # 只给文件名也行
```

日志目录是**项目共享**的（每条 tick 记着自己那份的相对路径，所以多目标不会串）。

#### `zloop doc [<id>]`

**干什么**：把多轮日志**合成一份**完整技术文档：目标、每条 todo、每一轮的思路 / 决策 / 坑 / 证据 / 改动文件。

**什么时候敲**：一个目标做完要交付、复盘、或者贴给别人看的时候。

| 参数 | 说明 |
|---|---|
| `<TODO>` | 只出这一条 todo 的全部轮次 |
| `--all` | 整个目标的每条 todo |
| `--last <N>` | 只要最近 N 轮（跨 todo 一起数） |
| `--since <TIME>` / `--until <TIME>` | 只要这段时间里的轮次；`2h` / `30m` / `7d`、`2026-08-29`、或完整 ISO 时间戳 |
| `--out <FILE>` | 写到文件，不打屏幕 |

```bash
$ zloop doc --all --out docs/CI-优化过程.md
$ zloop doc t2                     # 只看 t2 那几轮
$ zloop doc --all --last 20        # 跑了几十轮之后：只要最近 20 轮
$ zloop doc --all --since 3d       # 或者：这三天干了什么
```

不带范围参数就是全文，和以前一样。**一限范围，抬头就说清楚它省了什么**——一份只覆盖部分轮次的
文档长得和完整文档一模一样，不写明白就是在骗读它的人：

```
$ zloop doc --all --last 2
# 技术文档 · g1

**目标**：测试范围选择

生成于 2026-08-29T10:07:01+08:00 · 目标状态 done · 共 3 条 todo

> **范围**：最近 2 轮 —— 收录 2 轮，省略 1 轮（`zloop doc` 不带范围参数出全文）
```

范围外一轮都不剩的 todo 整章不出（否则 `--all --last 3` 还是会摊开几十章空标题）；只指名一条
todo 时那一章照出，抬头写着「收录 0 轮」，好让你知道是筛没了，不是这条 todo 没干过活。

#### `zloop sessions`

**干什么**：列出干过活的宿主会话——各做了哪些 todo、transcript 还在不在、**怎么 resume 回去**。

**什么时候敲**：想进到当时那个会话里看细节的时候（"第 3 轮它到底看了什么才这么改"）。

| 参数 | 说明 |
|---|---|
| `--host claude\|codex\|cli` | 只看某个宿主 |
| `--json` | 机器可读 |

```
$ zloop sessions
claude 11111111-2222-3333-4444-555555555555  ticks 7   2026-08-28T20:15:11+08:00 → 2026-08-28T20:41:07+08:00  todos t1,t2  ✓ transcript
        claude --resume 11111111-2222-3333-4444-555555555555
```

第二行就是可以直接抄走的 resume 命令。transcript 被清理掉的会话会标 `transcript missing`——`--resume` 还能试，但看不到当时的对话。

#### `zloop context`

**干什么**：生成一个**有界**的交接包：目标、最近三轮、下一条 todo、待办、经验、会话、怎么继续。默认压在 4000 字符内。

**什么时候敲**：换宿主（Claude Code ↔ Codex）、开新会话、或者你自己想快速搞清现状的时候。模型每轮第一步也敲它。

| 参数 | 说明 |
|---|---|
| `--budget <N>` | 字符预算（默认 4000）。超了先砍历史，保留目标和下一步 |
| `--for claude\|codex\|cli` | 调整最后"怎么继续"那一行的措辞 |

`status` 里的 `阶段` 是压缩版；**完整那句英文 `phase` 在 `context` 和 `next --json` 里**——脚本认这个，不认人类视图。

#### `zloop doctor`

**干什么**：只读体检 `.zloop/`——逐条报出"问题 + 下一步该敲什么"。它找的是**不报错的不一致**：这些毛病平时一声不吭，只让某条命令在某一天突然不听话。

**什么时候敲**：`goal switch` 说"对上了 2 个目标"、某条 todo 永远排不上、`zloop doc` 少了一节、或者手工改过 `.zloop/` 之后。健康的项目输出一行 `没发现问题`。

| 参数 | 说明 |
|---|---|
| `--json` | 机器可读：`{goals, archived, errors, warnings, findings[{kind, level, what, fix}]}` |

查这些（`kind` 是稳定标识，可以用来在脚本里挑）：

| kind | 级别 | 什么情况 |
|---|---|---|
| `headless` | 要修 | 没有当前目标（搬家中断 / 归档掉了当前那个），目标其实都还在 `goals/` |
| `broken_goal` | 要修 | 目标文件读不出来（`goal list` 只显示"损坏"，不告诉你怎么办） |
| `id_filename_mismatch` | 要修 | `goals/<文件名>.json` 里的 id 和文件名对不上——下一次停放会按 id 再造一个同名文件 |
| `duplicate_goal_id` | 要修 | 两个文件抢同一个 id，这个 id 从此 `switch` / `rm` 都点不动 |
| `dangling_in_progress` | 要修 | 在飞的派活指着一条已经不存在的 todo，`done` 认不出它 |
| `dangling_blocked_by` | 要修 | 依赖指向不存在的 todo（手改过 `state.json`，或老版本 `compact` 搬走了被依赖的那条——今天的 `compact` 会留下它）——这条 todo 永远轮不到。`zloop status` 的清单里也标成 `⛔ 等不到 tN` |
| `dead_blocked_by` | 要修 | 依赖的那条 todo 还在清单里，但它已经派不出去了（已延后，或状态被手改成 zloop 不认的词）——依赖要 done 才放行，等它的那条同样永远轮不到。同样标成 `⛔ 等不到 tN` |
| `dep_cycle` | 要修 / 留意 | 依赖成了环（`t1 ← t2 ← t1`，自依赖也算）：环上每条都在等下一条先做完，谁都不会先动。环上还有活着的 todo 是「要修」，全了结掉了是「留意」（现在卡不住谁，捡回来就会） |
| `duplicate_todo_id` | 要修 | 同一个 todo id 有多条，`done` / `edit` 只改得到第一条 |
| `next_id_reuse` | 要修 | `next_id` 已经被用过，下一条 `plan` 会造出重复 id |
| `bad_policy` | 要修 / 留意 | `policy` 里的数值写出了范围：`window_hours` 不在 `0..=8760`、`max_total_usd` 为负（都是「要修」，取值被无声换掉），或 `intervals_min` 为空（「留意」，退回 3 分钟） |
| `unreadable_notes` | 要修 | `NOTES.md` 在、但读不出来（非 UTF-8 / 权限）——`context` 会**静默**少掉「约定」「经验」两整节，模型当轮没有任何项目护栏，命令还退 0 |
| `missing_log` | 留意 | tick 记着的日志文件被删了（信息没了，循环照跑） |
| `broken_archive` / `archive_id_collision` | 留意 | 归档文件读不出 / 归档里多份同名，只影响翻旧账 |
| `stale_pid` / `bad_pid_file` | 留意 | `runner/pid` 指着一个不在的进程（`status` / `stop` 会顺手清） |
| `leftover_tmp` | 留意 | 上次写入被打断留下的 `*.tmp`（正本没事：写法是 tmp → rename），没人清 |

```
$ zloop doctor

  体检 /path/to/proj/.zloop · 目标 2 个 · 归档 0 份

  ✗ .zloop/goals/renamed.json 里的 id 是 "alpha"，和文件名对不上
    → mv .zloop/goals/renamed.json .zloop/goals/alpha.json
  ✗ [alpha] t2 依赖 t1，但没有这条 todo——它永远轮不到
    → zloop edit t2 --blocked-by ''   # 或改成真实存在的 id
  ! .zloop/runner/pid 指着 pid 999999，这个进程已经不在了
    → `zloop status` 或 `zloop stop` 会顺手清掉它（doctor 只读，不动文件）

  3 个问题：2 个要修、1 个留意（doctor 只读，一个字都没改）
```

两条设计约定：

1. **只读是硬约束**。doctor 不修、不删、不动任何文件——连 `daemon::running()` 都不调用（它会顺手删掉过期的 pid 文件）。体检和治疗分开，你才敢在任何状态下敲它，包括 runner 正在跑的时候。
2. **退出码只认"要修"**：有 `要修` 级别才退 `1`，只有 `留意` 照样退 `0`。否则 CI 里一个被删掉的旧日志就能让流水线红一片。

### 后台长跑

#### `zloop start` / `zloop stop`

**干什么**：`start` 把 runner 放到后台（`setsid` 分离，关掉终端也不影响），pid 写在 `.zloop/runner/pid`，输出在 `.zloop/runner/console.log`。`stop` 给它 SIGTERM，让它把当前这轮收尾后退出。

**什么时候敲**：想让它自己跑几个小时的时候。这是长程任务的常规用法。

`start` 接受 [`run`](#zloop-run) 的全部参数。已经有 runner 在跑时再 `start` 会被拒（退出码 2）。

**起来就会秒退的，`start` 直接不起**（退出码 1）：启动前先跑一遍和 runner 第一轮一模一样的判断（`decide` + 等待策略），会立刻 `stop(...)` 的情况当场拒绝，并说清楚原因和下一步——比报告「started」再让 runner 在 console.log 里留一句 reason 强。等人（`user_gate` / `blocked`）是挂着轮询、不是秒退，照常起。

```bash
$ zloop start                     # 还没规划过
start: 没启动——runner 起来第一轮就会退出（unplanned）。
原因：这个目标一条待办都没有。
下一步：zloop plan --add "[P0] 第一件事"（或在 Claude Code 里 `/zloop <目标>` 让它规划）
```

```bash
$ zloop start --max-budget-usd 2.00
runner started in the background (pid 41287, host claude)
log:    /path/.zloop/runner/console.log
watch:  zloop status
stop:   zloop stop

$ zloop stop
stopped runner (pid 41287)
```

macOS 上 runner 活着期间会**顶住合盖休眠**（`caffeinate` + 有 sudoers 规则时 `pmset disablesleep`），它一退出就恢复默认。见 [`awake`](#zloop-awake-action) 和 `docs/KEEP-AWAKE.md`。

#### `zloop run`

**干什么**：前台跑 runner，每轮做同一件事：`preflight`（可选）→ `next` → 调宿主（`claude -p` / `codex exec`）→ 宿主自己 `done` → 记账 → 按 `interval_min` 睡 → 下一轮。

**什么时候敲**：想亲眼看着它跑、或者在 tmux / CI 里跑的时候。日常长跑用 `start` 更省事。

| 参数 | 默认 | 说明 |
|---|---|---|
| `--host claude\|codex` | `claude` | 谁来执行每一轮 |
| `--max-rounds <N>` | `0` | 跑几轮就停（`0` = 一直到调度器说停） |
| `--timeout-min <N>` | `30` | 单轮超过这么久就杀掉宿主，记一笔 `fail` |
| `--resume todo\|all\|none` | `todo` | 会话复用：同一条 todo 才 resume / 一直 resume / 每轮全新 |
| `--max-budget-usd <金额>` | — | 传给 `claude -p --max-budget-usd`，每轮的花费上限 |
| `--exit-on-wait` | 关 | 等人时直接退出，而不是按退避阶梯的末档慢速轮询 |
| `--git-commit` | 关 | 每个写回过的轮次之后自动 `git commit`；只装**这个 runner 起跑之后**变化的文件（排除 `.zloop/`），起跑时就脏着的在制品留着不动，拆不开的会打印出来 |
| `--allow-all` | 关 | 绕过宿主的权限询问（`--dangerously-skip-permissions` / `danger-full-access`） |
| `--fast` | 关 | 把"分钟"当"秒"，只用来演示和测试 |
| `--reflect-every <N>` | `0` | 每 N 个 todo 轮次插一轮回看（见 [`reflect`](#zloop-reflect)）；不占轮次、不改 NOTES |
| `--no-replan` | 关 | 关掉「写回之后按信号重估计划」（默认开；见 [`replan`](#zloop-replan)。命中信号才跑，只产出建议、绝不改 todo） |
| `--auto-replan` | 关 | 让重估那一轮**真的改计划**。护栏在代码里强制；自主改满 3 次、或连着两次把清单改长，就停机等人（`stop reason=replan_diverged`） |
| `--no-keep-awake` | 关 | 不碰睡眠设置 |

**它自己会处理的事**：宿主返回限流（429 / rate limit / overloaded）→ 不记失败，睡 `intervals_min` 的**末档**（默认 30 分钟）再试；宿主卡住 → 到 `--timeout-min` 杀掉并记 `fail`；等人 → 默认按同一个末档慢速轮询而不是退出；被 `kill -9` → 下次启动时 journal 里记一笔 `restart`。

> 「末档」是**最后一档**，不是最大的一档：退避阶梯走到头就停在那儿，`[3, 30, 10]` 的末档是 10。阶梯本该越走越慢，写成往回走的 `doctor` 会报 `bad_policy`（警告级）——runner 照写下来的顺序睡，不替人把顺序纠正过来。

### 环境接入

#### `zloop install`

**干什么**：把 zloop 接进宿主。只写带 `<!-- zloop-managed:v1 …-->` 标记的文件；**内容不同才写**（一样就打印 `kept`），遇到同名但不是它写的文件会拒绝覆盖。

**什么时候敲**：装好二进制之后一次；**以后升级了 zloop 也要再敲一次**，否则 skill 里还是旧规则。

| 参数 | 写什么 |
|---|---|
| `--claude` | `~/.claude/skills/zloop/SKILL.md` → Claude Code 里多一个 `/zloop` |
| `--codex` | `~/.codex/skills/zloop/SKILL.md` + `agents/openai.yaml` → Codex 里多一个 `$zloop` |
| `--claude-stop-hook` | 往 `~/.claude/settings.json` 的 `hooks.Stop` 加一条 `zloop hook-stop`（实验性，默认不装） |
| `--sudoers` | macOS：写 `/etc/sudoers.d/zloop-pmset`，让 runner 能关掉合盖休眠（会要一次密码） |
| `--force` | 托管区被手改过时也照写（那些改动会丢；用户区不受影响） |

**新开的**会话才会加载新 skill。

##### SKILL.md 是给你改的：用户区与托管区

skill 不该是"只能被工具写"的文件——[Warp 那边 skill 就是改进的载体](COMMANDS.md)，人改完走 PR 合进去，下一轮 agent 就继承。所以 zloop 把 SKILL.md 切成两半：

```markdown
---
name: "zloop"
…
---

<!-- zloop-managed:v1 fp=18b64d5b1c389b09 -->      ← 托管区从这里开始，指纹钉住内容
# zloop /zloop
…（每轮协议、决策规则，升级时由 install 更新）

<!-- zloop:user -->                                 ← 这行以下归你，install 永不改动
- 汇报用中文，两三句话，别列清单
- 写回时 --evidence 一律贴真实输出，不要转述
```

- **用户区**（`<!-- zloop:user -->` 之后）：`install` 原样搬过去，输出会告诉你保留了多少字节。
  注意这份 SKILL.md 是**全局**的，所以这里只写**跨项目都成立**的话；
  某个仓库特有的规矩（"done 之前跑 cargo test"）该走那个项目的 [`.zloop/NOTES.md` 约定](#zloop-remember-一句话)，
  否则它会跟着你去每一个项目。
- **托管区**：内容指纹记在标记行里。你要是直接改了托管区，下次 `install` **停下报错**而不是悄悄盖掉：

```
$ zloop install --claude
zloop: …/SKILL.md 的托管区被改过（指纹 18b64d5b1c389b09 → 72d72766099da9e7），install 不会悄悄盖掉它。
把你的改动移到 `<!-- zloop:user -->` 之后（那一段永远保留），或者加 --force 用模板覆盖。
```

- 旧版本装的 SKILL.md 没有指纹，第一次用新版 `install` 会照旧覆盖一次并打印一行说明，从那以后保护生效。
- `agents/openai.yaml` 用同一套（YAML 注释形式的标记），只是没有用户区——它是配置不是提示词。

#### `zloop awake [<action>]`

**干什么**：看 / 修 macOS 的睡眠保护状态。

**什么时候敲**：想确认"合盖之后任务还会不会跑"，或者发现机器不睡了想查是谁干的。

| 动作 | 说明 |
|---|---|
| （无参数）`status` | 打印当前状态：有没有 runner 持有、`SleepDisabled` 是几、sudoers 规则在不在 |
| `reconcile` | 修正陈旧状态：没有 runner 活着却 `SleepDisabled=1`，就恢复默认 |

```bash
$ zloop awake
sleep: default (lid-close protection ready; a running runner will enable it)
```

正常情况下 `status` 里**不会**有"睡眠"这一行——只有它有话要说时才出现（比如正顶着合盖不睡、或者状态不一致需要 `reconcile`）。

#### `zloop notify [<文本>]`

**干什么**：用 `policy.notify_url` / `policy.notify_cmd` 发一条消息。

**什么时候敲**：配完 webhook 之后试一下通不通。真正的通知是 runner 在"等你决定 / 停下来 / 目标完成"时自动发的。

```bash
$ zloop notify "试一下"
```

飞书自定义机器人的地址会被自动识别并用飞书的消息格式发。没配任何通道时它会直接告诉你没配。

### 内部

#### `zloop hook-stop`

**干什么**：Claude Code Stop hook 的入口，从 stdin 读 hook JSON。装了 `--claude-stop-hook` 之后，Claude 每次想停下来它都会被叫一次：当前目录有 `.zloop/` 且还有可执行 todo，就拦住并把下一轮协议塞回去；todo 做完、等你决定、连续失败时放行；没有 `.zloop/` 的目录里什么都不做。

**你不用手敲**。runner 拉起的宿主进程带着 `ZLOOP_RUNNER=1`，这个 hook 见到就直接放行——否则 runner 的每一轮都会被自己的 hook 再套一层循环。

这个标记会继承给宿主进程的所有子进程，`cargo test` 也不例外。所以测试里 spawn `zloop` 一律走 `common::scrub_ambient_env()`，把 `ZLOOP_RUNNER` / `CLAUDECODE` / `CLAUDE_CODE_SESSION_ID` / `CODEX_THREAD_ID` 全清掉，需要哪个由测试自己显式设——否则「zloop 在自己的 runner 里跑自己的测试」永远是红的。
