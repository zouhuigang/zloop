# zloop

**让 Claude Code / Codex 围着一个目标持续干活的最小调度器。**
一个 JSON 状态文件、一个 1.6 MB 的静态 Rust 二进制，不需要任何解释器、服务或后台守护。你给它一个目标和几条 todo，它一轮做一条、做完写回、该停就停、能接着跑就接着跑——跑多久都行。

它专门解决四件事（设计见 [docs/RUST-DESIGN.md](docs/RUST-DESIGN.md)，长程运行加固见 [docs/LONG-RUN-AUDIT.md](docs/LONG-RUN-AUDIT.md)）：

| 目标 | 怎么用 | 背后 |
|---|---|---|
| **任务长时间运行** | `zloop start` / `zloop stop` | 后台 runner 驱动 `claude -p` 或 `codex exec`，一轮一条 todo；宿主挂死会超时 kill，限流会退避，等你决定时慢速轮询不退出，进程被杀再 `start` 一次就续 |
| **在 Claude Code ↔ Codex 之间切换不丢上下文** | `zloop context` | 两个宿主读同一个 `.zloop/state.json`；`context` 输出 ≤4000 字符的交接包（目标 / 当前判断 / 下一条 / 待办 / 各宿主会话） |
| **执行过程留档** | `zloop done --approach …` → `zloop log` / `zloop doc` | 每轮生成一份分节技术文档：实现思路、关键决策、遇到的坑、验证证据、自动抓取的改动文件 |
| **回到当时的会话看细节** | `zloop sessions` | 每轮自动记下 `CLAUDE_CODE_SESSION_ID` / `CODEX_THREAD_ID`，直接给你 `claude --resume <id>` / `codex resume <id>` |
| **等你决定时叫你** | policy `notify_url` / `notify_cmd` | runner 进入等人、限流、停机时推一条飞书 / 任意命令通知；`zloop notify` 测试配置 |
| **每条 todo 留一份技术文档** | `zloop done … --approach/--decision/--pitfall` → `zloop doc` | 完成一条 todo 必须写清实现思路（默认强制），加上关键决策、踩过的坑、验证证据和自动抓取的改动文件；`zloop doc --all` 导出整个目标的技术文档 |

zloop 是对 [loopx](https://github.com/huangruiteng/loopx) 里"Claude Code / Codex 核心调度"那 20% 的重写：保留"状态 → 该不该跑 → 跑一条 → 写回 → 决定下一 tick"这条主干，砍掉多 agent、能力插件、仪表盘、飞书、30 种交互模式和 32 万行代码。为什么这么做见 [docs/loopx-principles.md](docs/loopx-principles.md) 与 [docs/loopx-scheduling-notes.md](docs/loopx-scheduling-notes.md)。之后又对照了 Anthropic 的长时 agent harness 指南、Ralph Wiggum loop、Beads、OpenHands 以及 Claude Code / Codex 原生的 `/goal`，借鉴了验收标准、成本闸、通知、经验记忆、git checkpoint、环境自检等做法，见 [docs/OPEN-SOURCE-REVIEW.md](docs/OPEN-SOURCE-REVIEW.md)。

与 Claude Code / Codex 自带的 `/goal`、`/loop until:` 的关系：那是**单个会话内**的"直到条件满足"循环；zloop 的价值在会话之外——状态在文件里、跨宿主、跨重启，每轮留档、能回看会话、runner 不依赖任一宿主 UI 存活、停机条件是确定性的而不是模型自判。两者可以叠着用。

---

## 目录

- [一、安装](#一安装)
  - [1. 前置条件](#1-前置条件)
  - [2. 编译并安装二进制](#2-编译并安装二进制)
  - [3. 接入 Claude Code / Codex](#3-接入-claude-code--codex)
  - [4. 验证安装](#4-验证安装)
  - [5. 升级与卸载](#5-升级与卸载)
  - [6. 安装常见问题](#6-安装常见问题)
- [二、使用](#二使用)
  - [1. 五个概念](#1-五个概念)
  - [2. 方式一：在 Claude Code 里用 `/zloop`](#2-方式一在-claude-code-里用-zloop)
  - [3. 方式二：后台长跑 `start` / `status` / `stop`](#3-方式二后台长跑-start--status--stop)
  - [4. 方式三：在 Codex 里用](#4-方式三在-codex-里用)
  - [5. 方式四：手动或脚本驱动](#5-方式四手动或脚本驱动)
  - [6. 看进度：`status` / `log` / `sessions` / `context`](#6-看进度status--log--sessions--context)
  - [6.1 每条 todo 留一份技术文档](#61-每条-todo-留一份技术文档)
  - [6.2 一个项目多个目标](#62-一个项目多个目标)
  - [7. 停下来了怎么办](#7-停下来了怎么办)
  - [8. 调参](#8-调参)
  - [9. 从 loopx 迁移](#9-从-loopx-迁移)
- [三、参考](#三参考)
  - [命令一览](#命令一览)
  - [命令详解](#命令详解)（每条命令干什么、什么时候敲、参数、例子）
  - [`next` 怎么决定](#next-怎么决定)
  - [`.zloop/` 目录与状态文件](#zloop-目录与状态文件)
  - [与 loopx 的对比](#与-loopx-的对比)
  - [明确不做](#明确不做)
  - [开发](#开发)

---

## 一、安装

### 1. 前置条件

| 需要 | 说明 |
|---|---|
| macOS 或 Linux | 后台运行用到 `setsid` / `flock`；Windows 未测试 |
| Rust ≥ 1.75 | 只在编译时需要。没有的话：`curl https://sh.rustup.rs -sSf \| sh`，或用 mise：`mise use -g rust@stable` |
| Claude Code CLI 和/或 Codex CLI | zloop 自己不调模型，它驱动这两个宿主。至少装一个并登录：`claude` 能进交互界面、`codex login status` 显示已登录 |

### 2. 编译并安装二进制

```bash
git clone https://github.com/zouhuigang/zloop.git && cd zloop
cargo build --release                                   # 产物 target/release/zloop（约 1.6 MB，静态单文件）
install -m755 target/release/zloop ~/.local/bin/zloop   # 放到 PATH 里任意目录都行
```

`~/.local/bin` 不在 PATH 的话，在 shell 配置里加一行 `export PATH="$HOME/.local/bin:$PATH"`（fish：`fish_add_path ~/.local/bin`）。

### 3. 接入 Claude Code / Codex

```bash
zloop install --claude      # 写 ~/.claude/skills/zloop/SKILL.md            → Claude Code 里多一个 /zloop
zloop install --codex       # 写 ~/.codex/skills/zloop/SKILL.md + agents/openai.yaml → Codex 里多一个 $zloop
```

两条都可以装，互不影响。`install` 只写带 `<!-- zloop-managed:v1 -->` 标记的文件：重复执行是幂等的（输出 `kept`），遇到同名但不是它写的文件会拒绝覆盖。**新开的 Claude Code / Codex 会话**才会加载新 skill。

可选——Claude Code Stop hook：

```bash
zloop install --claude-stop-hook   # 往 ~/.claude/settings.json 的 hooks.Stop 加一条 "zloop hook-stop"
```

装了它之后，在有 `.zloop/` 且还有可执行 todo 的项目里，Claude 每次要停下来时会被拦住并塞回下一轮协议，不用你敲 `/loop`；todo 全做完、等你决定、连续失败 3 次时自动放行；在没有 `.zloop/` 的目录里它什么也不做。停用：删掉 settings.json 里那条 hook。**默认不装**，因为它对所有目录生效，需要你知道自己在开什么。

**谁在场，hook 就闭嘴**（[#14](https://github.com/zouhuigang/zloop/issues/14)）。源码文件没有锁，两个 agent 同时改一批文件就是互相覆盖，所以这条队伍同一时刻只该有一个人在干活：

| 现场 | hook 的行为 | 判据 |
|---|---|---|
| 我自己就是 runner 起的 `claude -p` | 放行不催（一次调用 = 一轮，不链式接活） | `ZLOOP_RUNNER` 环境变量 |
| 有无头 runner 在跑 | **闭嘴** | `.zloop/runner/pid` 里的进程还活着 |
| 另一个交互会话拿着这一轮 | **闭嘴** | 和 `next` 同一个 `held_by_other` |
| 那一轮是我自己拿的 | 照常催——活本来就是我的 | 同上 |
| 分不出我是谁（裸 CLI，没有会话 id） | 照常催 | 拦了只会把人锁在门外 |

**runner 那两行为什么不复用 `held_by_other`**：因为 `held_by_other` 对 runner **必须**放行。runner 设完 `in_progress` 才去起 `claude -p`，那个子进程自己会敲 `zloop next`、带的是它自己的新会话 id——在 `held_by_other` 眼里就是"别人"。把 runner 也算进去，runner 就把自家子进程挡在门外了。所以那一半改看进程在不在。

runner 在**轮次之间睡觉**时同样闭嘴：它醒来就接着领活，这几分钟放交互会话进去只是把撞车挪个时刻。`zloop stop` 之后立刻恢复催活。

### 4. 验证安装

```bash
zloop --version                              # zloop 0.3.0
ls ~/.claude/skills/zloop/SKILL.md           # Claude Code skill 在位
ls ~/.codex/skills/zloop/agents/openai.yaml  # Codex skill 在位（如果装了）

mkdir /tmp/zl-check && cd /tmp/zl-check
zloop init "安装验证" && zloop plan --add "[P0] 打个招呼" && zloop next
# RUN  t1 [P0] 打个招呼
#      writeback: zloop done t1 --note '<一句话结果>'
#      interval: 3 min · remaining 1
#      phase: executing t1 · round 1 · since 07:20 (0s ago) · host cli · via next
zloop done t1 --note "hi" --no-doc && zloop status   # ✅ 完成  1/1 todo · 1 轮
cd - && rm -rf /tmp/zl-check
```

想验证宿主真的能被驱动（会调一次真实模型，约 1 分钟）：

```bash
mkdir /tmp/zl-real && cd /tmp/zl-real
zloop init "验证 runner" && zloop plan --add "[P0] 在项目目录创建 hello.txt，内容一行：hello zloop"
zloop run --host claude --max-rounds 1       # 看到 "runner: round 1 written back" 和 "session → claude --resume …"
cat hello.txt && zloop sessions               # hello zloop / claude <id> … ✓ transcript
```

### 5. 升级与卸载

```bash
# 升级
cd zloop && git pull && cargo build --release && install -m755 target/release/zloop ~/.local/bin/zloop
zloop install --claude --codex               # skill 模板有变化时会输出 wrote，否则 kept

# 卸载
zloop stop                                   # 每个还在后台跑的项目里各执行一次
rm ~/.local/bin/zloop
rm -rf ~/.claude/skills/zloop ~/.codex/skills/zloop
# 装过 Stop hook 的话，删掉 ~/.claude/settings.json 里 hooks.Stop 中 command 为 "zloop hook-stop" 的那条
# 各项目里的 .zloop/ 是你的运行记录，留不留随你
```

**升级只换托管区，用户区停在你装它那天。** `install` 把 `<!-- zloop:user -->` 之后的内容原样搬过去——
这是承诺，反过来也意味着**模板自带的那段用户区文字，只有全新安装才看得到**：

```
$ zloop install --claude
wrote  ~/.claude/skills/zloop/SKILL.md
       保留了你的自定义段落（<!-- zloop:user --> 之后 152 字节）   ← 152 字节是老模板的原文，不是新的
```

想要新版那段：自己照着改，或者备份后删掉 `SKILL.md` 再 `install`。

**runner 正在跑的时候能不能升级**：skill 那一半可以——runner 每轮的提示词来自二进制里的
`prompt::heartbeat`（`zloop heartbeat --host claude` 打出来的就是它，一字不差），**从不读 SKILL.md**，
所以重写 skill 影响不到在跑的这一轮。要等的是换二进制那一半：agent 每轮都在敲 `zloop context` /
`zloop done`，跑到一半换掉等于中途换工具——那一步放到 `zloop stop` 之后。

### 6. 安装常见问题

| 现象 | 原因 / 处理 |
|---|---|
| `zloop: command not found` | `~/.local/bin` 不在 PATH，见第 2 步 |
| `cargo: command not found` | 没装 Rust，见第 1 步；用 mise 装的话 cargo 在 `~/.local/share/mise/shims/` |
| Claude Code 里没有 `/zloop` | skill 只在新会话加载；确认 `~/.claude/skills/zloop/SKILL.md` 存在 |
| `zloop run --host codex` 报认证错误 | 先 `codex login` |
| `install` 报 `exists and is not managed by zloop` | 你自己在那个位置放过同名文件，备份后删掉再装 |
| Stop hook 装了没反应 | hook 配置只在新会话生效；且只在 cwd 向上能找到 `.zloop/` 时才起作用 |

---

## 二、使用

下面按**怎么用**组织：先讲清五个概念，再讲四种驱动方式，然后是看进度、停机处理、调参。**想查某条命令干什么、什么时候敲、有哪些参数，直接看[命令详解](#命令详解)。**

### 1. 五个概念

| 概念 | 是什么 | 在哪 |
|---|---|---|
| **goal** | 一个项目**当前**的目标，一句话；其余目标停在 `.zloop/goals/` | `.zloop/state.json` → `goal.text`；换目标用 `zloop goal new` / `goal switch`（见 [6.2](#62-一个项目多个目标)） |
| **todo** | 目标拆成的有序步骤，带 `[P0]/[P1]/[P2]` 优先级；状态 open / blocked / deferred / done | `zloop plan` 写入，`zloop done` / `zloop edit` 改 |
| **outcome** | 一条 tick 的结果。**干活的**：`done` / `progress` / `fail`（只有这三种算轮次、算配额）。**其余**：`block`（等人）、`noop`（该轮没跑）、`edit`（改了 todo）、[`feedback`](#zloop-feedback-todo-人说的)（**你**说的话）、[`reflect`](#zloop-reflect)（整理经验）、[`replan`](#zloop-replan)（重估计划） | `ticks[].outcome` |
| **轮（round）** | 一次"取一条 todo → 做 → 写回"。每轮只做一条 | `zloop next` 取，`zloop done` 写回 |
| **tick** | 每次写回留下的一条记录：时间、todo、结果、宿主、会话 | `state.json` → `ticks[]`，同时生成一个 `.zloop/log/*.md` |
| **phase** | 循环现在处于什么阶段：executing / sleeping / idle / waiting / stopped | `zloop status` 第 2 行，`zloop next --json` 的 `phase` 字段 |

一个项目一个 `.zloop/` 目录，放在项目根；所有命令从当前目录向上找它，也可以用 `--dir <路径>` 指定。

### 2. 方式一：在 Claude Code 里用 `/zloop`

最省事的入口。打开项目，在 Claude Code 里：

```
/zloop 把 demo 服务的启动时间降到 1 秒以内
```

Claude 会：`zloop init` 建目标 → 把目标拆成 2–5 条可验证的 todo 交给 `zloop plan` → 立刻跑第一轮（`zloop context` → `zloop next --json` → 做那一条 → `zloop done t1 --note … --evidence …`）→ 用两三句话汇报。

之后：

```
/zloop              ← 再跑一轮
/zloop status       ← 看状态（也可以是 context / sessions / log / next）
/loop /zloop        ← 让 Claude Code 自己按 interval_min 一轮轮续跑（Claude Code 内置的 /loop）
```

装了 Stop hook 的话连 `/loop` 都不用敲：只要还有可执行的 todo，Claude 想停下就会被拦回去继续。

### 3. 方式二：后台长跑 `start` / `status` / `stop`

不想守着终端、要跑几小时到几天的任务，用这个。

```bash
cd my-project
zloop init "…"                                   # 已经用 /zloop 建过就跳过
printf '[P0] …\n[P1] …\n[P2] …\n' | zloop plan   # 一行一条，可选 [Pn] 前缀，默认 P1

zloop start          # 后台开跑：默认用 claude，每轮 30 分钟超时；关终端、合盖都不影响
zloop status         # 第 2 行：循环到哪了；第 3 行：runner 在不在（pid）
zloop stop           # 停
```

`start` 做的事：用独立会话（setsid）重新执行 `zloop run`，输出写到 `.zloop/runner/console.log`，pid 记在 `.zloop/runner/pid`。重复 `start` 会拒绝（"already running"）。想看实时输出：`tail -f .zloop/runner/console.log`。

每一轮 runner 做什么：`next` 选一条 todo → 组装 prompt（每轮协议 + 当前 todo）→ 调 `claude -p`（或 `codex exec`）→ 等它执行完 → 检查它有没有 `zloop done` 写回 → 睡 `interval_min` → 下一轮。所有"该不该继续"的判断都来自 `next`，runner 自己不做决定。

常用参数（`start` 和 `run` 一样）：

```bash
zloop start --host codex            # 换 Codex 跑；两个宿主共享同一状态，可以交替
zloop start --max-budget-usd 2.00   # 每轮花费上限，透传给 claude -p（Codex 无此参数）
zloop start --timeout-min 60        # 单轮宿主超时（默认 30）
zloop start --resume all            # 会话续接：todo（默认，同一 todo 续接、换 todo 新会话）| all（一直续）| none（每轮全新）
zloop run --fast --max-rounds 3     # 前台跑、间隔按秒算、只跑 3 轮——演示和调试用
```

权限：默认只放行 `Bash(zloop:*)` + 读写编辑工具（Claude）/ `--sandbox workspace-write`（Codex）；加 `--allow-all` 才跳过全部权限确认。

**电脑重启、进程被杀之后，再 `zloop start` 一次就行**：journal 里会多一条 `restart`，从当前状态续跑，做完的 todo 不会重做。

### 3.1 合上盖子任务不停（macOS）

runner 活着的时候 Mac 不睡，runner 一停就恢复系统默认——这是 `zloop start` 的默认行为，不用配置：

| 层 | 做什么 | 需要什么 |
|---|---|---|
| `caffeinate -i -s -w <runner pid>` | 防空闲休眠、接电源时防系统休眠；runner 退出它自动退出 | 无 |
| `sudo pmset -a disablesleep 1` | **合上盖子也不睡**（`caffeinate` 和任何第三方断言都做不到这一点——Apple 从 10.13 起把对应的私有 API 锁给了自家进程） | 一次性配置免密：`zloop install --sudoers` |

```bash
zloop install --sudoers     # 写 /etc/sudoers.d/zloop-pmset，只放行 3 条精确的 pmset 命令，会要一次密码
zloop start                 # 之后每次 start 自动开、stop 自动关
zloop status                # 第 4 行 sleep: lid-close sleep disabled by zloop (1 runner) · restores when they stop
zloop awake                 # 单看睡眠状态和哪些 runner 在"举着"它
```

**什么时候恢复默认息屏？** 只看 runner 还在不在，**跟开不开盖子无关**（代码里没有任何盖子/唤醒事件的钩子）：

| 你做的事 | 结果 |
|---|---|
| 合盖 → 过一会儿开盖 | 任务继续跑，睡眠仍禁用。**不用敲任何命令** |
| 任务自己跑完（或因连续失败 / 原地踏步 / 超预算停机） | runner 退出，**自动**恢复默认息屏 |
| 你主动 `zloop stop` | 恢复默认息屏 |
| runner 被 `kill -9`、崩溃、机器重启 | 兜底恢复（Drop guard 立即恢复；强杀由 watchdog 15 秒内恢复；重启这种极端情况由下一次 `zloop start/status/awake` 修正） |

多个项目同时跑时按 runner 计数，停掉一个不会误关另一个。

不想动睡眠设置：`zloop start --no-keep-awake`。没配 sudoers 也能跑，只是合盖会睡，`status` 会提示。

**风险**：合盖不睡 = 放进包里也在跑，发热、掉电；`disablesleep` 跨重启持久，若 runner 运行中机器重启，开机后第一次 `zloop status` 会看到 ⚠ 并提示 `zloop awake reconcile`。长任务请插电。

### 4. 方式三：在 Codex 里用

- **Codex CLI / App 交互**：`$zloop`（Codex 不支持自定义顶层 slash），行为与 `/zloop` 相同。
- **Codex App automation**：让模型用 `automation_update` 建一条 automation，body 就是 `zloop heartbeat --host codex-app` 的输出，初始间隔 3 分钟；`interval_min` 为 `null` 时暂停。
- **Codex 原生 goal 循环**：`/goal <zloop heartbeat --host codex-cli 的输出>`。
- **后台 runner**：`zloop start --host codex`。

从 Claude Code 切到 Codex（或反过来）时，在新宿主里第一步 `zloop context`——它给你目标、最近三轮做了什么、下一条是什么、另一边的会话怎么回看。

### 5. 方式四：手动或脚本驱动

不经过任何 AI 宿主，把 zloop 当一个带节奏控制的 todo 队列用：

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
#  "writeback": "zloop done t1 --note '<一句话结果>'", "interval_min": 3,
#  "phase": "executing t1 · round 1 · since 07:20 (0s ago) · host cli · via next"}

zloop edit t3 --acceptance "冷启动 ≤1s，bench.sh 连跑 3 次都通过"   # 验收标准：模型 done 前要逐条自检；没带 --evidence 会被提醒
zloop done t1 --note "基线 3.2s，脚本 bench.sh" --evidence @bench.log   # 完成；--evidence 可以是文本或 @文件
zloop done t2 --outcome progress --note "已定位 2 个，第 3 个待查"        # 有进展但没完
zloop done t2 --block "第 3 个步骤涉及付费 SDK，是否允许替换？"            # 需要人决定；next 会跳过它去做 t3
zloop done t3 --note "懒加载完成" --next "[P1] 补一个启动耗时的回归测试"   # 完成并在其后插入一条新 todo
zloop edit t2 --status open                                              # 人回答了问题，把 t2 放回队列
zloop remember "bench.sh 要在 release 模式下跑，debug 数据差 3 倍"           # 经验进 .zloop/NOTES.md，下次 context 会带上
zloop feedback t3 "方向不对，别走这条路"                                   # 你的回应进账本，下一轮先处理它
zloop status
```

每轮协议——模型看到的就是这 5 条（`zloop heartbeat --host claude|codex-app|codex-cli` 打印，约 850 字符）：

```
1. 先运行 `zloop context` 读交接包，再运行 `zloop next --json`。交接包里有「用户对上一轮的反馈」就先按它调整这一轮的做法。
   should_run=false 时按 reason 简短告知用户后停止本轮。
2. should_run=true 时，只做 todo 里这一条：做出可验证的产物，能跑的就跑一下验证。
3. 完成 → `zloop done <id> --note "…" [--evidence "…"]`；有进展没做完 → --outcome progress；失败 → --outcome fail；
   需要用户决定 → --block "<问题>"；发现新任务 → --next "<任务>"；
   **这一轮的结论动摇了后续计划** → --rethink "<哪条前提不成立了>"（哪怕这一轮是成功的）。
   写回的输出里出现「计划可能要调整」时，跑一次 `zloop replan` 并把建议讲给用户；改 todo 要用户点头。
4. 不要改 .zloop/ 以外的调度状态；不碰凭证、不做破坏性 git、不做生产操作。
5. 每轮结束用两三句话告诉用户：做了什么、验证了什么、下一条是什么。
```

### 6. 看进度：`status` / `log` / `sessions` / `context`

`zloop status` 一屏回答三个问题：**现在在哪一步、还剩什么、我该敲什么。**

```
  ▶  就绪      ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    0/4 完成
  ┌──────┬────┬────────────────────────────┬──────────┐
  │ 步骤 │ id │ 这一步做什么               │ 进展     │
  ├──────┼────┼────────────────────────────┼──────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处   │ ▶ 下一个 │
  │      │    │ 验收：有火焰图和三个函数名 │          │
  │    2 │ t2 │ 给启动路径加 tracing       │ ○ 排队中 │
  │    3 │ t3 │ 把配置加载改成懒加载       │ ○ 排队中 │
  │    4 │ t4 │ 写压测脚本                 │ ○ 排队中 │
  └──────┴────┴────────────────────────────┴──────────┘

  阶段    没人在跑 · 下一条是 t1
  后台    没有 runner 在跑

  开跑    zloop start
```

跑起来之后——做过的打勾留在清单上，正在做的和被挡住的各自说清情况：

```
  🔄 执行中    ████░░░░░░░░░░░░ 25%  跑了 3 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    1/4 完成
  ┌──────┬────┬────────────────────────────────────────┬─────────────┐
  │ 步骤 │ id │ 这一步做什么                           │ 进展        │
  ├──────┼────┼────────────────────────────────────────┼─────────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处               │ ✅ 完成     │
  │    2 │ t2 │ 给启动路径加 tracing                   │ 🔄 执行中   │
  │    3 │ t3 │ 把配置加载改成懒加载                   │ ❗ 等你回话 │
  │      │    │ ↳ 懒加载会不会影响首屏？要不要加开关？ │             │
  │      │    │ 答完敲 zloop edit t3 --status open     │             │
  │    4 │ t4 │ 写压测脚本                             │ ⏳ 等 t3    │
  └──────┴────┴────────────────────────────────────────┴─────────────┘

  阶段    claude 正在做 t2 · 第 2 轮 · 已跑 0s
  后台    没有 runner 在跑
  会话    claude --resume 11111111-2222-3333-4444-555555555555

  写回    zloop done t2 --note "<一句话结果>" --approach "<怎么做的>"
```

**① 标题 + `目标` + `阶段` = 现在在哪一步。** 标题的状态词一共八种，颜色各不相同：

| 状态词 | 什么情况 | 颜色 |
|---|---|---|
| ✅ 完成 | 所有 todo 做完了 | 绿 |
| 🔄 执行中 | 某条 todo 正被执行（`next` 交出去了，还没写回） | 青 |
| 💤 休眠中 | 后台 runner 在两轮之间睡觉 | 蓝 |
| ▶ 就绪 | 有活可做，等你 `/zloop` 或 `zloop start` | 蓝 |
| ⏳ 等你决定 | 所有能干的活都被 `--block` 了，问题印在那一步下面 | 黄 |
| ⏱ 限流中 | 24 小时窗口内的次数（`max_runs`）用完了，在等窗口滑走 | 黄 |
| ⛔ 已停 | 连续失败 / 原地踏步 / 超预算 | 红 |
| ⏸ 已暂停 | 你 `zloop pause` 了 | 黄 |
| ◦ 待规划 | 目标刚开，还没有待办（`zloop goal new` 之后就是这个状态） | 蓝 |

`阶段` 那一行**任何状态都在**，说的是循环此刻在干什么：`claude 正在做 t2 · 第 2 轮 · 已跑 4m`、`两轮之间的休息 · 睡到 14:19 醒`、`等你回答 · 10 分钟后重试`、`连续失败，已停下等你处理`、`11 条待办全部完成，目标结束`。

**② `清单` = 还剩什么。** 一张四列的表，做完的不会消失、打上勾留在表里——复盘时最想看的就是"做过哪几步"。

| 列 | 是什么 |
|---|---|
| `步骤` | **执行顺序**，第几件事。折叠提示（`… 前 11 步已收起`）按这个数 |
| `id` | **命令里敲的那个词**（`zloop done t4`、`zloop doc t7`、`zloop log --todo t7`）。每一行都有 |
| `这一步做什么` | todo 文本；验收标准、被 `--block` 的问题、解锁命令都缩在它下面 |
| `进展` | 这一步现在什么情况，见下表 |

> **`步骤` 和 `id` 不是一回事**，别把 `t5` 读成"第 5 步"。id 按**创建顺序**发（t1…tN），而 `zloop done --next` 会把后继任务插在**当前这条的后面**——所以第 4 步完全可能是 `t8`。两列都印出来就是为了这个。

`进展` 一列的取值：

| 进展 | 含义 |
|---|---|
| `✅ 完成` | 做完了（做完的不再显示验收标准） |
| `🔄 执行中` | 正被某个会话拿着做 |
| `▶ 下一个` | 下一轮就做它（表按步骤顺序排，`next` 按优先级挑，所以"下一个"不一定是下一行） |
| `❗ 等你回话` | 被 `--block` 了；问题在 `↳`，**解锁命令就在它下面**（窄窗口下命令会整条印在表格下面，绝不裁一半） |
| `⏳ 等 t3` | 在等前置 todo，等哪条直接写出来 |
| `○ 排队中` | 排在后面，没被挡 |
| `⏭ 已延后` | `zloop edit t6 --status deferred` 挂起的。**不算进百分比的分母**——调度器把它当已了结，所以进度写成 `6/6 完成 · 2 条延后`，而不是 6/8 |

超过 15 步就折叠：没做完的全留着，前面垫 3 步做过的当上下文，其余收成 `… 前 11 步已收起`。

**③ 明细 + 页脚 = 我该敲什么。** 灰标签是情况，青标签是**可以直接抄走的命令**，一行一条：

- `目标` / `清单` / `阶段` / `后台` **永远在**；`其他`（还有几个目标停着）/ `睡眠` / `文档` / `会话` 只在有话要说时出现（睡眠设置正常、没有缺文档的轮次就不占行）；
- 页脚随状态变：就绪 → `开跑`；执行中 → `写回 zloop done t2 --note … --approach …`；等你决定 → 命令已贴在那一步下面；限流中 → `放宽`；已停 → `看失败` + `重跑`；已暂停 → `继续`；完成 → `加活` / `换目标` / `出文档`。

<details>
<summary>其余 6 种状态的实拍（休眠中 / 等你决定 / 限流中 / 已停 / 已暂停 / 完成）</summary>

**休眠中**

```
  💤 休眠中    ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    0/4 完成
  ┌──────┬────┬────────────────────────────┬──────────┐
  │ 步骤 │ id │ 这一步做什么               │ 进展     │
  ├──────┼────┼────────────────────────────┼──────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处   │ ▶ 下一个 │
  │      │    │ 验收：有火焰图和三个函数名 │          │
  │    2 │ t2 │ 给启动路径加 tracing       │ ○ 排队中 │
  │    3 │ t3 │ 把配置加载改成懒加载       │ ○ 排队中 │
  │    4 │ t4 │ 写压测脚本                 │ ○ 排队中 │
  └──────┴────┴────────────────────────────┴──────────┘

  阶段    两轮之间的休息 · 睡到 15:04 醒，还有 4m11s
  后台    没有 runner 在跑

  看日志  zloop log
```

**等你决定**

```
  ⏳ 等你决定  ████░░░░░░░░░░░░ 25%  跑了 4 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    1/4 完成
  ┌──────┬────┬─────────────────────────────────────────┬─────────────┐
  │ 步骤 │ id │ 这一步做什么                            │ 进展        │
  ├──────┼────┼─────────────────────────────────────────┼─────────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处                │ ✅ 完成     │
  │    2 │ t2 │ 给启动路径加 tracing                    │ ❗ 等你回话 │
  │      │    │ ↳ 第 3 步要用付费 SDK，能换成开源的吗？ │             │
  │      │    │ 答完敲 zloop edit t2 --status open      │             │
  │    3 │ t3 │ 把配置加载改成懒加载                    │ ❗ 等你回话 │
  │      │    │ ↳ 懒加载会不会影响首屏？要不要加开关？  │             │
  │      │    │ 答完敲 zloop edit t3 --status open      │             │
  │    4 │ t4 │ 写压测脚本                              │ ❗ 等你回话 │
  │      │    │ ↳ 压测跑在 CI 还是本地？                │             │
  │      │    │ 答完敲 zloop edit t4 --status open      │             │
  └──────┴────┴─────────────────────────────────────────┴─────────────┘

  阶段    等你回答 · 10 分钟后重试
  后台    没有 runner 在跑
  会话    claude --resume 11111111-2222-3333-4444-555555555555
```

**限流中**

```
  ⏱ 限流中    ░░░░░░░░░░░░░░░░ 0%  跑了 1 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    0/4 完成
  ┌──────┬────┬────────────────────────────┬──────────┐
  │ 步骤 │ id │ 这一步做什么               │ 进展     │
  ├──────┼────┼────────────────────────────┼──────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处   │ ○ 排队中 │
  │      │    │ 验收：有火焰图和三个函数名 │          │
  │    2 │ t2 │ 给启动路径加 tracing       │ ○ 排队中 │
  │    3 │ t3 │ 把配置加载改成懒加载       │ ○ 排队中 │
  │    4 │ t4 │ 写压测脚本                 │ ○ 排队中 │
  └──────┴────┴────────────────────────────┴──────────┘

  阶段    本窗口次数用完 · 约 1 天后重试
  后台    没有 runner 在跑

  放宽    改 .zloop/state.json 的 policy.max_runs（0 = 不限）
```

**已停（连续失败）**

```
  ⛔ 已停      ░░░░░░░░░░░░░░░░ 0%  跑了 3 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    0/4 完成
  ┌──────┬────┬────────────────────────────┬──────────┐
  │ 步骤 │ id │ 这一步做什么               │ 进展     │
  ├──────┼────┼────────────────────────────┼──────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处   │ ○ 排队中 │
  │      │    │ 验收：有火焰图和三个函数名 │          │
  │    2 │ t2 │ 给启动路径加 tracing       │ ○ 排队中 │
  │    3 │ t3 │ 把配置加载改成懒加载       │ ○ 排队中 │
  │    4 │ t4 │ 写压测脚本                 │ ○ 排队中 │
  └──────┴────┴────────────────────────────┴──────────┘

  阶段    连续失败，已停下等你处理
  后台    没有 runner 在跑
  会话    claude --resume 11111111-2222-3333-4444-555555555555

  看失败  zloop log
  重跑    zloop start
```

**已暂停**

```
  ⏸ 已暂停    ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    0/4 完成
  ┌──────┬────┬────────────────────────────┬──────────┐
  │ 步骤 │ id │ 这一步做什么               │ 进展     │
  ├──────┼────┼────────────────────────────┼──────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处   │ ○ 排队中 │
  │      │    │ 验收：有火焰图和三个函数名 │          │
  │    2 │ t2 │ 给启动路径加 tracing       │ ○ 排队中 │
  │    3 │ t3 │ 把配置加载改成懒加载       │ ○ 排队中 │
  │    4 │ t4 │ 写压测脚本                 │ ○ 排队中 │
  └──────┴────┴────────────────────────────┴──────────┘

  阶段    你按了暂停，待办原地保留
  后台    没有 runner 在跑

  继续    zloop resume
```

**完成**

```
  ✅ 完成      ████████████████ 100%  跑了 4 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  清单    4/4 完成
  ┌──────┬────┬──────────────────────────┬─────────┐
  │ 步骤 │ id │ 这一步做什么             │ 进展    │
  ├──────┼────┼──────────────────────────┼─────────┤
  │    1 │ t1 │ 找出启动路径上最慢的三处 │ ✅ 完成 │
  │    2 │ t2 │ 给启动路径加 tracing     │ ✅ 完成 │
  │    3 │ t3 │ 把配置加载改成懒加载     │ ✅ 完成 │
  │    4 │ t4 │ 写压测脚本               │ ✅ 完成 │
  └──────┴────┴──────────────────────────┴─────────┘

  阶段    4 条待办全部完成，目标结束
  后台    没有 runner 在跑
  会话    claude --resume 11111111-2222-3333-4444-555555555555

  加活    zloop plan --add "[P0] 下一件事"
  换目标  zloop goal new "另一个目标"
  出文档  zloop doc --all
```

</details>

**不会折行**：每行都按终端实际宽度（`TIOCGWINSZ`，也认 `COLUMNS`）裁剪，窄窗口先丢进度条、再收窄清单的文本列（表格框线跟着收，不会歪——列宽按 Unicode East_Asian_Width 算，中文和 emoji 各占两列）。只有两类不裁：`会话` 的 resume 命令和青色的命令行；清单里的解锁命令塞不进表格时，会整条印在表格下面而不是裁一半。

**颜色什么时候开**：输出到终端时开；管道、重定向、`NO_COLOR=1`、`--no-color` 时自动关，纯文本可直接 `grep`。`CLICOLOR_FORCE=1` 可强制开（比如想 `less -R`）。

```bash
zloop status                  # 上面这一屏
zloop status --no-color       # 纯文本
zloop status --json           # 整份状态，脚本用这个而不是 grep 人类视图
zloop status --md             # Markdown 投影（含每条 tick 的 resume 命令和 log 链接），可重定向到文件给人看
zloop log                     # 最近 20 轮留档（时间倒序）
zloop log --todo t2           # 某条 todo 的每一轮
zloop log --show 20260827-071049-t3-done.md
zloop sessions                # 出现过的宿主会话、各做了哪些 todo、transcript 是否还在、怎么 resume
zloop context                 # 交接包：换宿主 / 开新会话 / 想快速搞清现状时先看它
```

`status` 里的 `阶段` 是压缩版；完整那句在 `zloop context` 和 `zloop next --json` 的 `phase` 字段里（脚本认这个，不认人类视图）：

| `phase` | 含义 |
|---|---|
| `executing t3 · round 4 · since 06:20 (3m ago) · host claude · via next` | 一条 todo 已被交出去（`next` 或 runner），还没 `done` |
| `executing … ⚠ stale (>120m, …)` | 交出去两小时还没写回，那个会话多半没了 |
| `runner sleeping until 06:41 (2m10s left) · reason ready` | runner 在两轮之间睡觉 |
| `runner round 4 on t2 since … — no end recorded` | runner 开了一轮没收尾（进程可能死了），再 `start` 会续 |
| `idle · next would run t4 [P1] …` | 没人在跑，下一轮会做 t4 |
| `waiting (user_gate) · retry in 10 min` | 暂时没活可干，在退避 |
| `stopped (done \| user_gate \| fail_streak \| progress_streak \| paused)` | 循环已停，原因在括号里 |

### 6.1 每条 todo 留一份技术文档

一条 todo 做完，光有"做完了"没用——三个月后（或者换个人接手）真正想知道的是**怎么做的、当时为什么这么选、踩过哪些坑**。所以 `zloop done` 完成一条 todo 时**默认要求**带上实现思路：

```bash
zloop done t3 --note "基线 3.2s" \
  --approach "先写 bench.sh 连跑 3 次取中位数避免抖动；只在 release 下测" \
  --decision "不引入 criterion，多一个依赖不值当" \
  --decision "基线脚本进仓库，后续回归直接复用" \
  --pitfall  "第一次用 debug 跑出 9.8s，比 release 慢 3 倍，白排查半小时" \
  --evidence "bench.sh: 3.19s / 3.22s / 3.20s（median 3.20s）"
```

`--decision` 和 `--pitfall` 可以重复；`--approach` / `--evidence` 支持 `@文件`。生成的 `.zloop/log/<时间>-t3-done.md` 是一份分节文档：

```markdown
# t3 · done · 2026-08-28T08:08:43+08:00
- goal / todo / acceptance / outcome / round / host / session / resume / cost / note
## 实现思路      ← --approach
## 关键决策      ← --decision（可重复）
## 遇到的坑      ← --pitfall（可重复）
## 验证证据      ← --evidence
## 改动文件      ← 自动：git diff --stat + 未跟踪文件，排除 .zloop/
```

忘了写会怎样：

```
$ zloop done t3 --note "做完了"
done: t3 完成时需要留下技术文档（policy.require_doc）。带上实现思路再重试，例如：
  zloop done t3 --note "<一句话结果>" \
    --approach "<怎么做的、为什么这么做>" \
    ...
确实不需要文档：加 --no-doc；想永久关闭：把 .zloop/state.json 的 policy.require_doc 设为 false。
```

装人写的正文的那些参数（`--note` / `--approach` / `--decision` / `--pitfall` / `--evidence` / `--block` / `--next`，
以及 `init` / `plan --add` / `remember` / `feedback` / `goal new` 的文本）都允许以 `-` 开头——
记「哪个 flag 不该用」这种坑时经常要写 `--force 会归档旧目标`。打错的 flag **值**照旧会报错。

拒绝发生在写状态之前，所以这次调用什么都没改，补上参数重跑即可。`--outcome progress` 和 `--block` 不强制——没做完的轮次写不出完整思路。

**失败那一轮欠的是另一样东西：坑。** `--outcome fail` 必须带 `--pitfall`（`policy.require_pitfall`，默认开）：

```
$ zloop done t3 --outcome fail --note "链接失败"
done: t3 这一轮失败了，得留下踩到的坑（policy.require_pitfall），否则下次还会踩。带上 --pitfall 再重试：
  zloop done t3 --outcome fail --note "<一句话：卡在哪>" \
    --pitfall "<试了什么、为什么不行、下次该从哪切入>" \
    --evidence "<报错输出，或 @文件>"
```

为什么要卡这一下：连续失败会让循环**停下来等人**，但"停下来"不等于"学到"——原因没有落点的话，
下一轮（甚至下一个会话）会把同一个坑再踩一遍。记下的坑同时写进账本（`tick.pitfalls`）和日志文件，
于是 `zloop context` 每轮都会带上一节：

```
## 本目标失败过的地方（别重复踩）
- 2026-08-29T06:48:47+08:00 t1 失败：cargo build 在 M1 上链接失败
  ↳ 坑：sqlite3 要用 brew 那份 libsqlite3，系统自带的缺符号；下次先 otool -L 看链的是哪个
- 2026-08-29T06:49:00+08:00 t2 卡住：压测跑 CI 还是本地？
```

这一节排在「下一条」**前面**，篇幅超预算时也不会被裁掉。runner 自己记的失败（preflight 不过、宿主超时）
没有坑可写，就只显示那一行——它们是机械故障，不是判断失误。

导出：

```bash
zloop doc t3                       # 这条 todo 的所有轮次，合成一份文档
zloop doc --all --out docs/TECH.md # 整个目标：概览 + 每条 todo 一章 + 每轮一节
zloop doc --all --last 20          # 跑长了就限个范围：最近 20 轮（或 --since 3d）
zloop log                          # 只有结果记录、没有实现思路的轮次会打 ⚠
```

### 6.2 一个项目多个目标

**当前**目标躺在 `.zloop/state.json`，其余的停在 `.zloop/goals/<id>.json`。切换就是把当前那份停走、把目标那份开进来（两步都是原子 rename），所以"同一时刻只有一个目标在跑"这条不变量不变——runner、Stop hook、文件锁都不受影响。

```bash
zloop goal new "另一件事"        # 当前目标原地停放，开一个新的（不丢任何东西）
zloop goal list                  # 或 zloop goals：全部目标，▸ 是当前那个
zloop goal switch keep-awake     # 按 id 切
zloop goal switch 冷启动         # 也能按目标文字里的片段切（歧义时会让你说清）
zloop goal rm keep-awake         # 归档：从 list 消失，文件搬到 .zloop/archive/
zloop goal rm 冷启动             # 片段也能对上，但会先打出对上的是谁、等你敲 y
zloop goal rm g1 --yes           # 脚本里免问（-y 同）
```

```
  共 2 个目标 · ▸ 是当前那个
  ▸ multi-goal  进行中    0/3  08-28 20:18  让 zloop 支持一个项目多个目标…
    zloop       完成    12/12  08-28 15:02  看看安装好了吗?然后检查一下…
                ↑ 停着但没做完的显示「停放」，读不出来的显示「损坏」

  切换  zloop goal switch <id 或目标里的片段>
  新建  zloop goal new "新目标"
```

- **id 怎么来**：从目标文字里的英文词拼（`让 keep-awake 支持外接显示器` → `keep-awake`），纯中文目标退到 `g1` / `g2`；也可以 `zloop goal new "…" --id my-slug` 自己指定。
- **停放 ≠ 归档**：停放的还在 `goal list` 里、`goal switch` 可切回；归档的（`.zloop/archive/`）不在 list 里，只留给事后翻。`zloop init --force` 是归档式覆盖，**换目标别用它**。
- **`goal rm` 猜出来的匹配要先问一句**：`switch` 和 `rm` 认的都是"id → id 前缀 → 目标文字片段"，但切错了再切回来就行，搬走一个目标不是。所以只有**精确 id** 免问；按前缀或文字片段对上时，先打印将要归档的是哪一个（id、目标全文、进度），再等一句 `y`——回车、`n`、别的都算不同意，退 1 且一个文件都不动。`--yes` / `-y` 跳过。stdin 读到 EOF（`</dev/null`、runner 里跑的）不当成"不同意"悄悄退，而是明说这一步要确认并给出 `--yes`。
- **切换前会挡你**：runner 在跑（会让它中途换活），或有会话拿着 todo 还没写回（切走那一轮就悬空了）——两种情况都拒绝并告诉你出路，确实要硬来加 `--force`。
- **`--force` 之后写回不会串目标**：硬切时会当场提醒"某条还在别的会话手里"；那个会话再 `zloop done` 会被拦下，并告诉它先切回原目标。确实要记在当前目标：`zloop done <id> --force`。
- **一条 todo 不会同时派给两个会话**：`zloop next` 发现这一轮已经派给别的会话且还没超过 `policy.stale_after_min`（默认 120 分钟），就报 `held_by_other` 并说清在谁手里，不抢占、不记 tick。超时后自动可以重派；把 `stale_after_min` 设成 0 关掉这个保护。**Stop hook 走同一道闸**——不然会出现 `next` 说"不给你派活"、hook 同时说"去做 t1"的自相矛盾（见下面「谁在场，hook 就闭嘴」）。
- **搬家是一个事务**：校验（id 合法/没被占、runner 空闲、没有轮次悬空）全在动文件之前；拿不到锁或中途失败会把停走的那份搬回来。所以不会出现"旧目标已经停走、新目标又没开起来"的空档。
- **万一真的没有当前目标**（历史遗留状态、或手工搬过文件）：`zloop goal list` 照样列出停着的目标并给出 `zloop goal switch <id>`；其他命令的报错也指这条路，不会让你 `init` 把目标埋掉。
- **读不出来的目标不会消失**：损坏或版本不匹配的目标在 list 里显示"损坏"（而不是静默隐藏），可以 `goal rm` 清掉，也不挡着你 `goal new` 开个干净的接着干。
- **跟着目标走的**：todo、tick、in_progress、policy、进度。**项目共享的**：`.zloop/log/` 的技术文档、`.zloop/NOTES.md` 的经验、`.zloop/runner/` 的 pid 与日志。`zloop log` 和 `zloop doc` 都认 tick 记的路径，所以只列当前目标的轮次；别的目标的日志文件还在磁盘上，`log` 会在末尾如实告诉你藏了几份。
- 刚开的目标还没有待办时，`status` 是 `◦ 待规划`，不是"全部完成"。

`/zloop 新目标` 也走这条路：skill 看到当前目标已完成、或新输入明显是另一件事，就 `zloop goal new`；如果当前目标还有没做完的 todo，它会先告诉你现状再问你要接着做还是开新目标。
**刚 `goal new` 完、一条待办都没有的目标是单独一支**：skill 直接给它 `zloop plan`，不会再 `goal new` 出一个重名的把它停放掉。
这一支在三个出口上都认得出来：`next --json` / `context` 报的 reason 是 **`unplanned`**（不是 `all_done`），`context` 的待办节写"还没有待办：先 zloop plan"，`status` 是 `◦ 待规划`。
`all_done` 只留给"有过 todo、现在全了结了"——那才该开新目标。两件事出口动作相反，所以不共用一个词（#5）。

#### 边界：goals 只看得见当前项目（这是取舍，不是缺陷）

`goals` / `status` / `context` 都是从当前目录往上找 `.zloop/`（`state.rs` 的 `find_root`），找到哪个项目就只认那个项目。
几个项目各自停放的目标，只能 `cd` 进去一个个看——**站在它们的父目录里问也没用**，zloop 不会往下扫：

```
$ cd /tmp/zl-t11/projA && zloop goals
  共 2 个目标 · ▸ 是当前那个
  ▸ g1     进行中  0/0  08-29 11:01  A 的第二件事：清理死代码
    200ms  停放    0/1  08-29 11:01  把 A 项目的冷启动降到 200ms      ← 只有 A 的

$ cd /tmp/zl-t11 && zloop goals            # 父目录，底下明明有 projA / projB
这个项目还没有目标：`zloop init "目标"`
```

全局那一层今天只有一个 `~/.zloop/awake/`（keep-awake 的持有者计数，见 [docs/KEEP-AWAKE.md](docs/KEEP-AWAKE.md)），
**没有任何项目或目标的索引**（[DESIGN.md](docs/DESIGN.md) 的 G1）。为什么这么定：

- **全局 registry 的成本全在一致性上**：loopx 为此写了 842 行（`global_registry.py`）——同步、merge、
  冲突路由、退休清理，外加 4 个 global skill。zloop 换目标就是一次 rename，没有第二份真源，也就没有对不齐的问题。
- **索引一定会烂**：项目会改名、搬走、删掉，索引里的那行不会自己消失。烂掉的索引最难受的地方是，
  你只会在**真的想用它**的那一刻才发现它在列不存在的项目。
- **这个视图人一般不缺**：你知道自己手上有哪几个项目；真要一眼扫完，下面这行 shell 就够，不值得为它引入一份全局状态。

**今天已经有的手动路径**（实测输出就是上面那两段）：

```bash
for d in ~/work/*/; do [ -d "$d/.zloop" ] && (cd "$d" && echo "== $(basename "$d")" && zloop goals); done
```

#### 如果要做：最小形态（没实现，[#8](https://github.com/zouhuigang/zloop/issues/8)）

一句话：**只记「项目在哪」，目标内容一律现场去各项目读。**

| 决定 | 怎么定 | 为什么 |
|---|---|---|
| 存哪 | `~/.zloop/projects.jsonl`，一行一个 `{root, last_seen}` | 全局状态都收在 `~/.zloop/`（`awake/` 已经在那儿）；一行一条，追加就行，不用读改写 |
| 谁写 | `init` 和 `goal new` 时追加（root 已在就更新 `last_seen`） | 只有"这个项目开始被 zloop 管"这一刻值得记；`next` / `done` 这种每轮都跑的命令一个字不写 |
| 记什么 | **只记 root 和时间**，目标文字、进度、todo 一律不记 | 不同步内容，就不会不同步——这是跟 loopx 那 842 行的分水岭 |
| 怎么读 | `zloop goals --all` 挨个进去读 `state.json` + `goals/`；读不到的当场标「已消失」并从索引里剔掉 | 索引允许烂，但每次读都自愈；真源永远是各项目自己的 `.zloop/` |
| 怎么显示 | 现在这张表加一列项目名，按 `last_seen` 倒序 | 复用 `cli.rs` 的 `row_status_zh`，不引入第二种输出格式 |
| 不做什么 | `--all` **只读**：不能跨项目 `switch` / `plan` / `done` | 跨项目的写才是 merge / 冲突路由那套负担的来源；读没有这个问题 |

改动量估计：新模块 `registry.rs`（追加 + 读取 + 剔除失效，约 60 行）、`cli.rs` 的 `goals` 加一个 `--all` 分支（约 40 行）、
`init` / `goal new` 各一行——约 100 行代码 + 3 个测试（重复 `init` 不留两行 / `--all` 能列出两个项目 / 项目删掉后自动剔除）。

**什么时候才做**：等你同时在跑三个以上 zloop 项目，并且真因为"忘了 B 项目还停着一个没做完的目标"吃过亏。
在那之前，上面那行 shell 就是最小形态。

### 7. 停下来了怎么办

下表左列是 `zloop context` / `zloop next --json` 里的 `phase` 字段（`status` 显示的是它的压缩版）：

| `phase` / runner 输出 | 发生了什么 | 你要做的 |
|---|---|---|
| `stopped (done)` | 全部 todo 完成 | 同一件事继续做：`zloop plan --add …` 加活会自动回到 active；**换一件事**：`zloop goal new "新目标"`（旧目标原地停放，[6.2](#62-一个项目多个目标)） |
| `waiting (user_gate)` / runner 日志 `polling until a human unblocks` | 某条 todo 被 `--block` 等你决定；后台 runner **没有退出**，在 30 分钟一次慢速轮询 | `zloop status` 看 `!` 那条下面 `↳` 的问题 → 回答后 `zloop edit t3 --status open`（必要时 `--text` 改写），runner 下次轮询自动续 |
| `stopped (fail_streak)` | 连续 3 轮失败：宿主超时、没写回、或真的报错 | `zloop log --todo t3` 看原因 → 修环境 / 拆 todo / `zloop edit t3 --text …`（任意 `edit` 都会重置计数）→ `zloop start` |
| `stopped (progress_streak)` | 同一 todo 连续 8 轮"有进展"却没完成 | 多半 todo 太大：`zloop edit t3 --text "更小的一步"` 或 `zloop done t3 --outcome progress --next "拆出的下一步"` |
| `stopped (paused)` | 你 `zloop pause` 了 | `zloop resume` |
| `stopped (budget)` | 累计花费达到 policy `max_total_usd` | 看 `zloop status` 标题行的 `$花费/上限`，确认值得就调大上限再 `start` |
| `waiting (throttled) · retry in N min` | 24 小时内已跑满 `max_runs`（默认 480） | 确实需要更快就调大 policy 的 `max_runs`，或设 `0` 不限 |
| runner 日志 `host rate-limited · not counted · sleeping 30 min` | 宿主返回 429 / rate limit / overloaded | 不用管，30 分钟后自动重试，不计失败 |
| runner 日志 `TIMED OUT (recorded fail)` | 某轮宿主超过 `--timeout-min` 被 kill | 偶发不用管；频繁出现就调大 `--timeout-min` 或把 todo 拆小 |
| `start: 没启动——runner 起来第一轮就会退出（…）` | `start` 的启动前体检：这一轮起来也是秒退，所以没起（退出码 1） | 照它第二、三行说的做（0 待办就 `zloop plan`，做完了就 `zloop goal new`，等等），再 `start` |

### 8. 调参

**调度策略**在 `.zloop/state.json` 的 `policy`，直接编辑：

| 字段 | 默认 | 含义 |
|---|---|---|
| `intervals_min` | `[3, 10, 30]` | 有活时每 3 分钟一轮；等人/无活时 10 → 30 分钟退避；30 也是 runner 等人时的轮询周期 |
| `max_runs` | `480` | 24 小时窗口内最多记账多少轮（done / progress / fail），防空转刹车；`0` 不限 |
| `window_hours` | `24` | 上面那个窗口的长度 |
| `max_fail_streak` | `3` | 连续失败几轮停下等人 |
| `max_noop_streak` | `3` | 交互式 `next` 连续几次"没活"后停止退避（runner 不受此影响） |
| `max_progress_streak` | `8` | 同一 todo 连续几轮 progress 没 done 就停；`0` 关闭 |
| `stale_after_min` | `120` | `in_progress` 多久没写回算悬挂 |
| `max_total_usd` | `0`（不限） | 本目标累计花费上限（来自 `claude -p` 返回的 `total_cost_usd`），达到即 `stopped (budget)` |
| `notify_url` | 无 | 通知 webhook。飞书自定义机器人地址会自动用飞书消息格式 |
| `notify_cmd` | 无 | 通知命令（`sh -c`），事件 JSON 从 stdin 进，另有 `ZLOOP_EVENT` / `ZLOOP_TEXT` / `ZLOOP_ROOT` 环境变量 |
| `preflight_cmd` | 无 | runner 每轮开始前先跑它（如 `./init.sh && cargo test`）；失败记一笔 `fail` 不调宿主，通过则把摘要放进 prompt |
| `require_doc` | `true` | 完成一条 todo 必须带 `--approach`（见 [6.1](#61-每条-todo-留一份技术文档)）；设为 `false` 关闭强制 |
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

### 9. 从 loopx 迁移

```bash
cd my-project
zloop init "$(grep '^objective:' .codex/goals/<goal>/ACTIVE_GOAL_STATE.md | cut -d'"' -f2)"
zloop plan --from-loopx .codex/goals/<goal>/ACTIVE_GOAL_STATE.md
```

只导入未勾选的 `- [ ] [Pn] …` 行（User Todo 与 Agent Todo 两节都算），剥掉 `<!-- loopx:todo … -->` 注释，`[P0]/[P1]/[P2]` 前缀原样保留；已完成 `[x]` 和延后 `[-]` 的不导入。loopx 的 `claimed_by`、`task_class`、`action_kind`、lease、successor 链等元数据没有对应物，直接丢弃。

---

## 三、参考

### 命令一览

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

### 命令详解

#### 开局

##### `zloop init "<goal>"`

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

##### `zloop goal` / `zloop goals`

**干什么**：一个项目里管多个目标。当前目标在 `.zloop/state.json`，其余停在 `.zloop/goals/<id>.json`；切换就是把当前那份停走、把目标那份开进来。详见 [6.2](#62-一个项目多个目标)。

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

##### `zloop plan`

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

#### 每一轮

##### `zloop next`

**干什么**：回答两个问题——**现在该不该跑**，以及**跑哪一条 todo**。这是整个调度器的入口。

**什么时候敲**：模型每轮开头敲，runner 每轮开头也敲。你自己一般不需要，想看"下一轮会做什么"用 `--peek`。

它做三件事：按 [`next` 决策梯](#next-怎么决定)算出 `should_run`；把选中的 todo **交出去**（`phase` 变成"执行中"，写 `in_progress`）；没活可干时记一笔 `noop`（用于退避计数）。

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

##### `zloop done <id>`

**干什么**：**唯一的写回口**。记一笔 tick、改 todo 状态、写这一轮的技术文档，必要时插入后继 todo。

**什么时候敲**：每轮结束时，做了什么就写什么。别的命令都不写执行历史。

| 参数 | 说明 |
|---|---|
| `--note <一句话>` | 结果摘要，会出现在 `status`、`context`、日志头部 |
| `--outcome done\|progress\|fail` | 默认 `done`。`progress` = 有进展但没完（todo 留着）；`fail` = 这轮失败（连续 3 次会停下来） |
| `--block <问题>` | 卡在你身上：todo 标成"等你回话"，问题原文印在 `status` 里 |
| `--next <LINE>` | 顺手插一条后继 todo，排在这条后面 |
| `--approach <文本\|@文件>` | **实现思路**：怎么做的、为什么这么做。`outcome=done` 时必填（见 [6.1](#61-每条-todo-留一份技术文档)） |
| `--decision <文本>` | 关键决策 / 取舍，可重复 |
| `--pitfall <文本>` | 遇到的坑与结论，可重复 |
| `--evidence <文本\|@文件>` | 验证证据：命令输出、测试名、测量值 |
| `--no-doc` | 这一轮不写技术文档（绕过 `policy.require_doc` 和 `policy.require_pitfall`） |
| `--force` | 派活来自别的目标时也照记到当前目标。默认会拦下来，让你先 `zloop goal switch <原目标>`（见 [6.2](#62-一个项目多个目标)） |

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

##### `zloop heartbeat`

**干什么**：打印"这一轮要遵守的 5 条协议"，给模型看。它自己不改任何状态。

**什么时候敲**：`/zloop` 无参数时，skill 第一件事就是敲它。

| 参数 | 说明 |
|---|---|
| `--host claude\|codex-app\|codex-cli` | 按宿主调整措辞（默认 `claude`） |

协议就是那五步：`zloop context` → `zloop next --json` → **只做那一条 todo** → `zloop done <id> …` → 两三句话汇报。

#### 中途调整

##### `zloop edit <id>`

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

##### `zloop pause` / `zloop resume`

**干什么**：把整个目标按住 / 放开。`pause` 之后 `next` 一律说"停"，后台 runner 在下一次检查时自己退出；todo 一条不动。

**什么时候敲**：临时要用机器干别的、或者想让它先别动的时候。比 `zloop stop` 更"硬"——`stop` 只是停掉 runner，你 `/zloop` 还能跑一轮；`pause` 是连人工那一轮也不给跑。

```bash
$ zloop pause
goal is now paused
$ zloop resume
goal is now active
```

##### `zloop stats`

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
| 轮次 | `done` + `progress` + `fail` 的 tick 数（`block` / `noop` / `edit` / `feedback` / `reflect` / `replan` 都不算）；`zloop status` 标题上的「跑了 N 轮」用的是同一个定义 |
| 返工 | `progress` + `fail` 的轮数；括号里是它占轮次的比例 |
| 一次过 | 一轮做完、中间没返工过的 todo 数 ÷ 已完成的 todo 数 |
| 无文档 | `documented == false` 的轮次（`zloop log` 里带 ⚠ 的那些） |
| 最费劲 | 返工最多的那条，其次看失败、被挡 |
| 花费 | 只在宿主报过 `cost_usd` 时才出现（交互式轮次没有这个数） |

`--json` 给脚本用，字段和上表一一对应，另有每条 todo 的明细。

**为什么会有这个命令**：Warp 的自改进回路是 **跑 → 打分 → 自改进**，`RunScorer` 就在自改进的前一环
（见 [`docs/SELF-IMPROVEMENT.md`](docs/SELF-IMPROVEMENT.md)）。zloop 此前只有"有没有留实现思路"这一个布尔值，
`stats` 是把打分这一环补上——它同时是 reflect 的输入。

##### `zloop replan`

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
[`docs/ADAPTIVE-REPLAN.md`](docs/ADAPTIVE-REPLAN.md)。

**改不改你点头**：`replan` 只给建议，落地走现成的 `zloop plan --add` / `zloop edit`。
提示词里还专门写了一句「**不用改是完全合格的结论**」，防止为了改而改。

**无头也有**：`zloop start` 默认开着这个——写回之后如果信号命中，runner 会插一轮重估，
**只把建议记进账本**（`zloop log` 里看得到），**绝不自己动 todo**。`--no-replan` 关掉。

---

###### 让它自己改：`zloop replan --apply` 与 `--auto-replan`

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
[`docs/ADAPTIVE-REPLAN.md`](docs/ADAPTIVE-REPLAN.md) §6–§10。

##### `zloop reflect`

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
### t1 · 2026-08-29T07:35
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

### 升格成约定（1 条）

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
（见 [`docs/SELF-IMPROVEMENT.md`](docs/SELF-IMPROVEMENT.md)）。

##### `zloop feedback <todo> "<人说的>"`

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
- **fail / noop / progress 三条 streak 都会被它打断**：循环因为连续失败停下来等人，人开口说话正是它该等到的东西。
  实测连续 3 次 `fail` 之后 `next` 是 `WAIT (fail_streak)`，`zloop feedback …` 之后立刻变回 `RUN`。
- 不吃配额、不推进轮次（`feedback` 不在计数的 outcome 里）；不改 todo 状态、不碰在飞状态。要让一条已完成的
  todo 重做，照旧是 `zloop edit <id> --status open`。

反馈跟着**目标**走（存在 `state.json` 里），所以多目标之间不会串。
处理过的反馈（后面又有 `done` / `progress` 轮次）不再占交接包版面，但一直留在 `zloop doc` 里。

##### `zloop remember "<一句话>"`

**干什么**：往 `.zloop/NOTES.md` 记一条经验。最新几条会自动出现在 `zloop context` 里。

**什么时候敲**：发现一个模型总是踩的坑、或者一条项目特有的约定时。这是纠正长程任务里反复犯的错最省力的办法。

```bash
$ zloop remember "fmt check 在 CI 上偶发失败，重跑即可，不要改代码"
remembered → /path/.zloop/NOTES.md

$ zloop remember --rule "done 之前一定要跑 cargo test"
约定 +1（共 1 条，每轮都带给模型）→ /path/.zloop/NOTES.md
```

经验是**项目共享**的，不跟着目标走。

###### 两层：约定 vs 经验

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

###### 边界：经验和约定都不跨项目（这是取舍，不是缺陷）

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

###### 如果要做：最小形态（没实现，[#9](https://github.com/zouhuigang/zloop/issues/9)）

一句话：**再开一份同格式的全局 NOTES，只做「约定」那一层。**

| 决定 | 怎么定 | 为什么 |
|---|---|---|
| 存哪 | `~/.zloop/NOTES.md` | 全局状态都收在 `~/.zloop/`（`awake/` 已经在那儿，[跨项目视图](#如果要做最小形态没实现8)那份设计也落在这里）；解析仍走同一份 `notes.rs`，不引入第二种格式 |
| 带哪层 | **只带约定，不带经验** | 经验只有最新 5 条的窗口；全局经验会跟项目经验抢这 5 条，而且抢赢了也讲不出道理 |
| 怎么写 | `zloop remember --global --rule "<一句话>"` | 多一个 flag，不多一条命令 |
| 怎么注入 | `context` 里排在项目约定**前面**，行首标「全局」 | 模型要能一眼分清"这条到处都成立"和"这条只在这个仓库成立" |
| 怎么管 | `reflect` 的材料里把全局约定单列一段，可以建议"升到全局 / 从全局降回项目"，`--apply` 按小标题写回各自的文件 | 保持"模型建议 → 人点头"这条唯一的落地路径 |
| 上限 | 全局比项目更严：项目 10 条，全局给 5 条；[reflect 的第三项体检](#zloop-reflect)把两边加起来一起算预算占比 | 全局约定是**每个项目每轮**都在付的钱 |

改动量估计：`notes.rs` 把路径参数化 + 一个 `global_path()`、`cli.rs` 一个 flag、`context.rs` 多注入一段、
`reflect.rs` 的体检多算一份——约 100 行代码 + 3 个测试（写得进全局 / 注入时两段都在且分得清 / 全局超限时体检出声）。

**什么时候才做**：等你在第二、第三个项目里发现自己在重敲同一条 `remember`。那时候需求是真的，
手上也正好有"它在几个项目里都成立"的证据。在那之前，多这一层只是多一个会过时的地方。

##### `zloop compact`

**干什么**：把很久以前完成 / 延后的 todo 和它们的 tick 搬进 `.zloop/archive/`，让 `state.json` 保持小、让 `status` 保持短。

**什么时候敲**：目标跑了几十轮、`status` 的清单开始翻页的时候。

| 参数 | 说明 |
|---|---|
| `--keep-days <N>` | 完成在 N 天内的留着（默认 7） |

技术文档（`.zloop/log/*.md`）不会被搬走，永远留在原地。

#### 看情况

##### `zloop status`

**干什么**：一屏回答三个问题——**现在在哪一步 / 还剩什么 / 我该敲什么**。详见 [6](#6-看进度status--log--sessions--context)。

**什么时候敲**：任何时候。它是只读的。

| 参数 | 说明 |
|---|---|
| `--json` | 整份状态（脚本用这个，别 grep 人类视图） |
| `--md` | Markdown 投影：每条 tick 带 resume 命令和日志链接，可重定向成文件给人看 |
| `--no-color` | 纯文本（管道 / 重定向时自动就是纯文本） |

##### `zloop log`

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

##### `zloop doc [<id>]`

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

##### `zloop sessions`

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

##### `zloop context`

**干什么**：生成一个**有界**的交接包：目标、最近三轮、下一条 todo、待办、经验、会话、怎么继续。默认压在 4000 字符内。

**什么时候敲**：换宿主（Claude Code ↔ Codex）、开新会话、或者你自己想快速搞清现状的时候。模型每轮第一步也敲它。

| 参数 | 说明 |
|---|---|
| `--budget <N>` | 字符预算（默认 4000）。超了先砍历史，保留目标和下一步 |
| `--for claude\|codex\|cli` | 调整最后"怎么继续"那一行的措辞 |

`status` 里的 `阶段` 是压缩版；**完整那句英文 `phase` 在 `context` 和 `next --json` 里**——脚本认这个，不认人类视图。

##### `zloop doctor`

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
| `dangling_blocked_by` | 要修 | 依赖指向不存在的 todo（`compact` 把被依赖的那条搬走就会这样）——这条 todo 永远轮不到 |
| `duplicate_todo_id` | 要修 | 同一个 todo id 有多条，`done` / `edit` 只改得到第一条 |
| `next_id_reuse` | 要修 | `next_id` 已经被用过，下一条 `plan` 会造出重复 id |
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

#### 后台长跑

##### `zloop start` / `zloop stop`

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

##### `zloop run`

**干什么**：前台跑 runner，每轮做同一件事：`preflight`（可选）→ `next` → 调宿主（`claude -p` / `codex exec`）→ 宿主自己 `done` → 记账 → 按 `interval_min` 睡 → 下一轮。

**什么时候敲**：想亲眼看着它跑、或者在 tmux / CI 里跑的时候。日常长跑用 `start` 更省事。

| 参数 | 默认 | 说明 |
|---|---|---|
| `--host claude\|codex` | `claude` | 谁来执行每一轮 |
| `--max-rounds <N>` | `0` | 跑几轮就停（`0` = 一直到调度器说停） |
| `--timeout-min <N>` | `30` | 单轮超过这么久就杀掉宿主，记一笔 `fail` |
| `--resume todo\|all\|none` | `todo` | 会话复用：同一条 todo 才 resume / 一直 resume / 每轮全新 |
| `--max-budget-usd <金额>` | — | 传给 `claude -p --max-budget-usd`，每轮的花费上限 |
| `--exit-on-wait` | 关 | 等人时直接退出，而不是按最慢间隔慢速轮询 |
| `--git-commit` | 关 | 每个写回过的轮次之后自动 `git commit`；只装**这个 runner 起跑之后**变化的文件（排除 `.zloop/`），起跑时就脏着的在制品留着不动，拆不开的会打印出来 |
| `--allow-all` | 关 | 绕过宿主的权限询问（`--dangerously-skip-permissions` / `danger-full-access`） |
| `--fast` | 关 | 把"分钟"当"秒"，只用来演示和测试 |
| `--reflect-every <N>` | `0` | 每 N 个 todo 轮次插一轮回看（见 [`reflect`](#zloop-reflect)）；不占轮次、不改 NOTES |
| `--no-replan` | 关 | 关掉「写回之后按信号重估计划」（默认开；见 [`replan`](#zloop-replan)。命中信号才跑，只产出建议、绝不改 todo） |
| `--auto-replan` | 关 | 让重估那一轮**真的改计划**。护栏在代码里强制；自主改满 3 次、或连着两次把清单改长，就停机等人（`stop reason=replan_diverged`） |
| `--no-keep-awake` | 关 | 不碰睡眠设置 |

**它自己会处理的事**：宿主返回限流（429 / rate limit / overloaded）→ 不记失败，睡 30 分钟再试；宿主卡住 → 到 `--timeout-min` 杀掉并记 `fail`；等人 → 默认按最慢间隔慢速轮询而不是退出；被 `kill -9` → 下次启动时 journal 里记一笔 `restart`。

#### 环境接入

##### `zloop install`

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

###### SKILL.md 是给你改的：用户区与托管区

skill 不该是"只能被工具写"的文件——[Warp 那边 skill 就是改进的载体](#三参考)，人改完走 PR 合进去，下一轮 agent 就继承。所以 zloop 把 SKILL.md 切成两半：

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

##### `zloop awake [<action>]`

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

##### `zloop notify [<文本>]`

**干什么**：用 `policy.notify_url` / `policy.notify_cmd` 发一条消息。

**什么时候敲**：配完 webhook 之后试一下通不通。真正的通知是 runner 在"等你决定 / 停下来 / 目标完成"时自动发的。

```bash
$ zloop notify "试一下"
```

飞书自定义机器人的地址会被自动识别并用飞书的消息格式发。没配任何通道时它会直接告诉你没配。

#### 内部

##### `zloop hook-stop`

**干什么**：Claude Code Stop hook 的入口，从 stdin 读 hook JSON。装了 `--claude-stop-hook` 之后，Claude 每次想停下来它都会被叫一次：当前目录有 `.zloop/` 且还有可执行 todo，就拦住并把下一轮协议塞回去；todo 做完、等你决定、连续失败时放行；没有 `.zloop/` 的目录里什么都不做。

**你不用手敲**。runner 拉起的宿主进程带着 `ZLOOP_RUNNER=1`，这个 hook 见到就直接放行——否则 runner 的每一轮都会被自己的 hook 再套一层循环。

这个标记会继承给宿主进程的所有子进程，`cargo test` 也不例外。所以测试里 spawn `zloop` 一律走 `common::scrub_ambient_env()`，把 `ZLOOP_RUNNER` / `CLAUDECODE` / `CLAUDE_CODE_SESSION_ID` / `CODEX_THREAD_ID` 全清掉，需要哪个由测试自己显式设——否则「zloop 在自己的 runner 里跑自己的测试」永远是红的。

### `next` 怎么决定

```
paused/done  >  unplanned / all_done  >  user_gate / blocked  >  fail_streak  >  progress_streak  >  throttled  >  ready
```

- 有可执行 todo（`open` 且 `blocked_by` 全部 done）→ `ready`，选 `(priority, 写入顺序)` 最靠前的一条，`interval_min = 3`。
- 一条 todo 都没有 → `unplanned`（去 `zloop plan`）；有过 todo 但全了结了 → `all_done`（去开新目标）。两者都是 `interval_min = null`，但下一步不一样，所以不共用一个词。
- 全部 blocked 且有人在等 → `user_gate`；纯依赖未满足 → `blocked`。退避 10 → 30 分钟，交互式连续 3 次 noop 后 `interval_min = null`。
- 最近连续 3 次 `fail` → `fail_streak`；同一 todo 连续 8 次 `progress` → `progress_streak`；两者都停下等人，`edit` 重置。
- 24 小时窗口内记账满 `max_runs` → `throttled`，给出几分钟后释放。

### `.zloop/` 目录与状态文件

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
  "next_id": 3,
  "updated_at": "…"
}
```

写入是 `tmp → fsync → rename` 原子替换；JSON 是唯一真源，`status --md` 只渲染、不回读。建议把 `.zloop/` 加进项目的 `.gitignore`——它是这台机器上的运行记录。

### 与 loopx 的对比

| 维度 | loopx 0.5.2 | zloop 0.2 |
|---|---|---|
| 源码文件 / 行数 | 819 / 317,699（Python） | 22 / ≈6,727（Rust） |
| 顶层子命令 | 113（叶命令 307） | 22（+1 内部 `hook-stop`）——每条的用途见[命令详解](#命令详解) |
| 单命令最多 flag | 75（`todo`） | 10（`run`） |
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

### 明确不做

多 agent 协作、能力路由、仪表盘、聊天、飞书、事件溯源、全局跨项目 registry、PreToolUse 强制拦截、开机自启。两个 agent 同时跑同一个 `.zloop/` 不会写坏文件，但会互相覆盖进度——这是有意为之。

经验与约定也**不跨项目继承**（理由和"真要做的话最小长什么样"见[边界：经验和约定都不跨项目](#边界经验和约定都不跨项目这是取舍不是缺陷)）；
同理，[`zloop goals` 只看得见当前项目](#边界goals-只看得见当前项目这是取舍不是缺陷)——几个项目的目标没有一个合起来的视图。

### 开发

```bash
cargo test                       # 71 个用例（tick / todo / state / cli / runner，runner 用假宿主，约 2 分钟）
cargo build --release && install -m755 target/release/zloop ~/.local/bin/zloop
```

目录：`src/` 实现 · `tests/` 集成测试 · `docs/`：`RUST-DESIGN.md` 当前设计、`LONG-RUN-AUDIT.md` 长程加固审计、`OPEN-SOURCE-REVIEW.md` 开源方案对照与借鉴、`TEST-REPORT.md` 自测报告、`loopx-principles.md` / `loopx-scheduling-notes.md` loopx 研究、`DESIGN.md` v0.1 Python 原型设计记录。

MIT License.
