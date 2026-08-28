# zloop

**让 Claude Code / Codex 围着一个目标持续干活的最小调度器。**
一个 JSON 状态文件、一个 1.2 MB 的 Rust 二进制、零运行时依赖。你给它一个目标和几条 todo，它一轮做一条、做完写回、该停就停、能接着跑就接着跑——跑多久都行。

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
cargo build --release                                   # 产物 target/release/zloop（约 1.2 MB，静态单文件）
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

### 4. 验证安装

```bash
zloop --version                              # zloop 0.2.0
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
| **goal** | 一个项目当前的目标，一句话 | `.zloop/state.json` → `goal.text`；换目标用 `zloop init --force`，旧的自动归档 |
| **todo** | 目标拆成的有序步骤，带 `[P0]/[P1]/[P2]` 优先级；状态 open / blocked / deferred / done | `zloop plan` 写入，`zloop done` / `zloop edit` 改 |
| **轮（round）** | 一次"取一条 todo → 做 → 写回"。每轮只做一条 | `zloop next` 取，`zloop done` 写回 |
| **tick** | 每次写回留下的一条记录：时间、todo、结果（done / progress / fail / block / noop / edit）、宿主、会话 | `state.json` → `ticks[]`，同时生成一个 `.zloop/log/*.md` |
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
zloop status
```

每轮协议——模型看到的就是这 5 条（`zloop heartbeat --host claude|codex-app|codex-cli` 打印，约 850 字符）：

```
1. 先运行 `zloop context` 读交接包，再运行 `zloop next --json`。should_run=false 时按 reason 简短告知用户后停止本轮。
2. should_run=true 时，只做 todo 里这一条：做出可验证的产物，能跑的就跑一下验证。
3. 完成 → `zloop done <id> --note "…" [--evidence "…"]`；有进展没做完 → --outcome progress；失败 → --outcome fail；
   需要用户决定 → --block "<问题>"；发现新任务 → --next "<任务>"。
4. 不要改 .zloop/ 以外的调度状态；不碰凭证、不做破坏性 git、不做生产操作。
5. 每轮结束用两三句话告诉用户：做了什么、验证了什么、下一条是什么。
```

### 6. 看进度：`status` / `log` / `sessions` / `context`

`zloop status` 一屏回答三个问题：**现在在哪一步、还剩什么、我该敲什么。**

```
  ▶  就绪      ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    0/4 完成
  1. 找出启动路径上最慢的三处    t1 ▶ 下一个
       验收：有火焰图和三个函数名
  2. 给启动路径加 tracing        t2 ○ 排队中
  3. 把配置加载改成懒加载        t3 ○ 排队中
  4. 写压测脚本                  t4 ○ 排队中

  阶段    没人在跑 · 下一条是 t1
  后台    没有 runner 在跑

  开跑    zloop start
```

跑起来之后——做过的打勾留在清单上，正在做的和被挡住的各自说清情况：

```
  🔄 执行中    ████░░░░░░░░░░░░ 25%  跑了 3 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    1/4 完成
  1. 找出启动路径上最慢的三处                 ✅
  2. 给启动路径加 tracing           t2 🔄 执行中
  3. 把配置加载改成懒加载          t3 ! 等你回话
       ↳ 懒加载会不会影响首屏？要不要加开关？
       答完敲 zloop edit t3 --status open
  4. 写压测脚本                  t4 ⏳ 等第 3 步

  阶段    claude 正在做 t2 · 第 2 轮 · 已跑 0s
  后台    没有 runner 在跑
  会话    claude --resume 36346c2a-1f3d-4b8e-9a52-0c7e5d21b4aa

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

**② `步骤` 清单 = 还剩什么。** 做完的不会消失，打上勾留在清单里——复盘时最想看的就是"做过哪几步"。每一步右侧是它自己的状态：

| 右栏 | 含义 |
|---|---|
| `✅` | 做完了（做完的不再显示 id，也不再显示验收标准） |
| `t2 🔄 执行中` | 正被某个会话拿着做 |
| `t2 ▶ 下一个` | 下一轮就做它（清单按步骤顺序排，`next` 按优先级挑，所以"下一个"不一定是下一行） |
| `t3 ! 等你回话` | 被 `--block` 了；问题在 `↳`，**解锁命令就在它下面** |
| `t4 ⏳ 等第 3 步` | 在等前置步骤，等哪一步直接写出来 |
| `t5 ○ 排队中` | 排在后面，没被挡 |
| `t6 ⏭ 已延后` | `zloop edit t6 --status deferred` 挂起的 |

超过 15 步就折叠：没做完的全留着，前面垫 3 步做过的当上下文，其余收成 `…  前 11 步已收起`。没做完的那些右侧带 id，因为 `zloop edit` / `zloop log --todo` 要用它。

**③ 明细 + 页脚 = 我该敲什么。** 灰标签是情况，青标签是**可以直接抄走的命令**，一行一条：

- `目标` / `步骤` / `阶段` / `后台` **永远在**；`其他`（还有几个目标停着）/ `睡眠` / `文档` / `会话` 只在有话要说时出现（睡眠设置正常、没有缺文档的轮次就不占行）；
- 页脚随状态变：就绪 → `开跑`；执行中 → `写回 zloop done t2 --note … --approach …`；等你决定 → 命令已贴在那一步下面；限流中 → `放宽`；已停 → `看失败` + `重跑`；已暂停 → `继续`；完成 → `加活` / `换目标` / `出文档`。

<details>
<summary>其余 6 种状态的实拍（休眠中 / 等你决定 / 限流中 / 已停 / 已暂停 / 完成）</summary>

**休眠中**

```
  💤 休眠中    ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    0/4 完成
  1. 找出启动路径上最慢的三处    t1 ▶ 下一个
       验收：有火焰图和三个函数名
  2. 给启动路径加 tracing        t2 ○ 排队中
  3. 把配置加载改成懒加载        t3 ○ 排队中
  4. 写压测脚本                  t4 ○ 排队中

  阶段    两轮之间的休息 · 睡到 15:04 醒，还有 4m11s
  后台    没有 runner 在跑

  看日志  zloop log
```

**等你决定**

```
  ⏳ 等你决定  ████░░░░░░░░░░░░ 25%  跑了 4 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    1/4 完成
  1. 找出启动路径上最慢的三处               ✅
  2. 给启动路径加 tracing        t2 ! 等你回话
       ↳ 第 3 步要用付费 SDK，能换成开源的吗？
       答完敲 zloop edit t2 --status open
  3. 把配置加载改成懒加载        t3 ! 等你回话
       ↳ 懒加载会不会影响首屏？要不要加开关？
       答完敲 zloop edit t3 --status open
  4. 写压测脚本                  t4 ! 等你回话
       ↳ 压测跑在 CI 还是本地？
       答完敲 zloop edit t4 --status open

  阶段    等你回答 · 10 分钟后重试
  后台    没有 runner 在跑
  会话    claude --resume 36346c2a-1f3d-4b8e-9a52-0c7e5d21b4aa
```

**限流中**

```
  ⏱ 限流中    ░░░░░░░░░░░░░░░░ 0%  跑了 1 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    0/4 完成
  1. 找出启动路径上最慢的三处    t1 ○ 排队中
       验收：有火焰图和三个函数名
  2. 给启动路径加 tracing        t2 ○ 排队中
  3. 把配置加载改成懒加载        t3 ○ 排队中
  4. 写压测脚本                  t4 ○ 排队中

  阶段    本窗口次数用完 · 约 1 天后重试
  后台    没有 runner 在跑

  放宽    改 .zloop/state.json 的 policy.max_runs（0 = 不限）
```

**已停（连续失败）**

```
  ⛔ 已停      ░░░░░░░░░░░░░░░░ 0%  跑了 3 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    0/4 完成
  1. 找出启动路径上最慢的三处    t1 ○ 排队中
       验收：有火焰图和三个函数名
  2. 给启动路径加 tracing        t2 ○ 排队中
  3. 把配置加载改成懒加载        t3 ○ 排队中
  4. 写压测脚本                  t4 ○ 排队中

  阶段    连续失败，已停下等你处理
  后台    没有 runner 在跑
  会话    claude --resume 36346c2a-1f3d-4b8e-9a52-0c7e5d21b4aa

  看失败  zloop log
  重跑    zloop start
```

**已暂停**

```
  ⏸ 已暂停    ░░░░░░░░░░░░░░░░ 0%  跑了 0 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    0/4 完成
  1. 找出启动路径上最慢的三处    t1 ○ 排队中
       验收：有火焰图和三个函数名
  2. 给启动路径加 tracing        t2 ○ 排队中
  3. 把配置加载改成懒加载        t3 ○ 排队中
  4. 写压测脚本                  t4 ○ 排队中

  阶段    你按了暂停，待办原地保留
  后台    没有 runner 在跑

  继续    zloop resume
```

**完成**

```
  ✅ 完成      ████████████████ 100%  跑了 4 轮
  目标    把 demo 服务的冷启动时间从 8 秒降到 1 秒以内

  步骤    4/4 完成
  1. 找出启动路径上最慢的三处    ✅
  2. 给启动路径加 tracing        ✅
  3. 把配置加载改成懒加载        ✅
  4. 写压测脚本                  ✅

  阶段    4 条待办全部完成，目标结束
  后台    没有 runner 在跑
  会话    claude --resume 36346c2a-1f3d-4b8e-9a52-0c7e5d21b4aa

  加活    zloop plan --add "[P0] 下一件事"
  换目标  zloop init --force "新目标"
  出文档  zloop doc --all
```

</details>

**不会折行**：每行都按终端实际宽度（`TIOCGWINSZ`，也认 `COLUMNS`）裁剪，窄窗口先丢进度条、再收窄文本列。只有两类不裁：`会话` 的 resume 命令和青色的命令行——半条命令比折行更糟。

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

拒绝发生在写状态之前，所以这次调用什么都没改，补上参数重跑即可。`--outcome progress|fail` 和 `--block` 不强制——没做完的轮次写不出完整思路。

导出：

```bash
zloop doc t3                       # 这条 todo 的所有轮次，合成一份文档
zloop doc --all --out docs/TECH.md # 整个目标：概览 + 每条 todo 一章 + 每轮一节
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
```

```
  共 2 个目标 · ▸ 是当前那个
  ▸ multi-goal  进行中    0/3  08-28 20:18  让 zloop 支持一个项目多个目标…
    zloop       完成    12/12  08-28 15:02  看看安装好了吗?然后检查一下…

  切换  zloop goal switch <id 或目标里的片段>
  新建  zloop goal new "新目标"
```

- **id 怎么来**：从目标文字里的英文词拼（`让 keep-awake 支持外接显示器` → `keep-awake`），纯中文目标退到 `g1` / `g2`；也可以 `zloop goal new "…" --id my-slug` 自己指定。
- **停放 ≠ 归档**：停放的还在 `goal list` 里、`goal switch` 可切回；归档的（`.zloop/archive/`）不在 list 里，只留给事后翻。`zloop init --force` 是归档式覆盖，**换目标别用它**。
- **切换前会挡你**：runner 在跑（会让它中途换活），或有会话拿着 todo 还没写回（切走那一轮就悬空了）——两种情况都拒绝并告诉你出路，确实要硬来加 `--force`。
- **跟着目标走的**：todo、tick、in_progress、policy、进度。**项目共享的**：`.zloop/log/` 里的技术文档（tick 记着自己那份的路径，所以不会串）、`.zloop/NOTES.md` 的经验、`.zloop/runner/` 的 pid 与日志。
- 刚开的目标还没有待办时，`status` 是 `◦ 待规划`，不是"全部完成"。

`/zloop 新目标` 也走这条路：skill 看到当前目标已完成、或新输入明显是另一件事，就 `zloop goal new`；如果当前目标还有没做完的 todo，它会先告诉你现状再问你要接着做还是开新目标。

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

**通知怎么配**（飞书群里加一个"自定义机器人"，拿到 webhook 地址）：

```bash
# 写进 .zloop/state.json 的 policy
"notify_url": "https://open.feishu.cn/open-apis/bot/v2/hook/xxxxxxxx"
zloop notify           # 群里收到"zloop 通知测试"即配置正确
```

之后 runner 在**等你决定**（某条 todo 被 `--block`）、**限流退避**、**停机**（除 `--max-rounds`）时各推一条，同一情形不重复。不用飞书的话 `notify_cmd` 随便接：`"notify_cmd": "osascript -e \"display notification \\\"$ZLOOP_TEXT\\\"\""`。

**runner 参数**（`start` / `run`）：`--host claude|codex`、`--timeout-min 30`、`--max-budget-usd`（仅 claude，单轮上限；总上限用 policy `max_total_usd`）、`--resume todo|all|none`、`--exit-on-wait`（等人时退出而不轮询，配合外部定时器用）、`--git-commit`（每轮写回后 `git commit`，排除 `.zloop/`）、`--max-rounds N`、`--fast`（间隔按秒，演示用）、`--allow-all`。

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

23 条命令，按用途分七组。**谁常敲**一列很重要：有些命令是给模型和 runner 用的，你平时不用碰。

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
| [`remember`](#zloop-remember-一句话) | 记一条经验，之后每轮都会带给模型 | 你 / 模型 |
| [`compact`](#zloop-compact) | 把老的完成项归档，state.json 保持小 | 你 |
| **看情况** | | |
| [`status`](#zloop-status) | 一屏：在哪一步 / 还剩什么 / 我该敲什么 | 你 |
| [`log`](#zloop-log) | 每轮的技术文档列表与内容 | 你 |
| [`doc`](#zloop-doc-id) | 把多轮日志合成一份完整文档 | 你 |
| [`sessions`](#zloop-sessions) | 出现过的宿主会话 + resume 命令 | 你 |
| [`context`](#zloop-context) | 有界交接包（换宿主 / 新会话先看它） | 模型 |
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

`rm` 只对停着的目标生效；要归档当前目标，先 `switch` 到别的。

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
| `--no-doc` | 这一轮不写技术文档（绕过 `policy.require_doc`） |

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

##### `zloop remember "<一句话>"`

**干什么**：往 `.zloop/NOTES.md` 记一条经验。最近几条会自动出现在 `zloop context` 里，也就是**每一轮都会带给模型**。

**什么时候敲**：发现一个模型总是踩的坑、或者一条项目特有的约定时。这是纠正长程任务里反复犯的错最省力的办法。

```bash
$ zloop remember "fmt check 在 CI 上偶发失败，重跑即可，不要改代码"
remembered → /path/.zloop/NOTES.md
```

经验是**项目共享**的，不跟着目标走。

##### `zloop compact`

**干什么**：把很久以前完成 / 延后的 todo 和它们的 tick 搬进 `.zloop/archive/`，让 `state.json` 保持小、让 `status` 保持短。

**什么时候敲**：目标跑了几十轮、`status` 的步骤清单开始翻页的时候。

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
| `--out <FILE>` | 写到文件，不打屏幕 |

```bash
$ zloop doc --all --out docs/CI-优化过程.md
$ zloop doc t2                     # 只看 t2 那几轮
```

##### `zloop sessions`

**干什么**：列出干过活的宿主会话——各做了哪些 todo、transcript 还在不在、**怎么 resume 回去**。

**什么时候敲**：想进到当时那个会话里看细节的时候（"第 3 轮它到底看了什么才这么改"）。

| 参数 | 说明 |
|---|---|
| `--host claude\|codex\|cli` | 只看某个宿主 |
| `--json` | 机器可读 |

```
$ zloop sessions
claude 36346c2a-1f3d-4b8e-9a52-0c7e5d21b4aa  ticks 7   2026-08-28T20:15:11+08:00 → 2026-08-28T20:41:07+08:00  todos t1,t2  ✓ transcript
        claude --resume 36346c2a-1f3d-4b8e-9a52-0c7e5d21b4aa
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

#### 后台长跑

##### `zloop start` / `zloop stop`

**干什么**：`start` 把 runner 放到后台（`setsid` 分离，关掉终端也不影响），pid 写在 `.zloop/runner/pid`，输出在 `.zloop/runner/console.log`。`stop` 给它 SIGTERM，让它把当前这轮收尾后退出。

**什么时候敲**：想让它自己跑几个小时的时候。这是长程任务的常规用法。

`start` 接受 [`run`](#zloop-run) 的全部参数。已经有 runner 在跑时再 `start` 会被拒（退出码 2）。

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
| `--git-commit` | 关 | 每个写回过的轮次之后自动 `git commit`（排除 `.zloop/`） |
| `--allow-all` | 关 | 绕过宿主的权限询问（`--dangerously-skip-permissions` / `danger-full-access`） |
| `--fast` | 关 | 把"分钟"当"秒"，只用来演示和测试 |
| `--no-keep-awake` | 关 | 不碰睡眠设置 |

**它自己会处理的事**：宿主返回限流（429 / rate limit / overloaded）→ 不记失败，睡 30 分钟再试；宿主卡住 → 到 `--timeout-min` 杀掉并记 `fail`；等人 → 默认按最慢间隔慢速轮询而不是退出；被 `kill -9` → 下次启动时 journal 里记一笔 `restart`。

#### 环境接入

##### `zloop install`

**干什么**：把 zloop 接进宿主。只写带 `<!-- zloop-managed:v1 -->` 标记的文件；**内容不同才写**（一样就打印 `kept`），遇到同名但不是它写的文件会拒绝覆盖。

**什么时候敲**：装好二进制之后一次；**以后升级了 zloop 也要再敲一次**，否则 skill 里还是旧规则。

| 参数 | 写什么 |
|---|---|
| `--claude` | `~/.claude/skills/zloop/SKILL.md` → Claude Code 里多一个 `/zloop` |
| `--codex` | `~/.codex/skills/zloop/SKILL.md` + `agents/openai.yaml` → Codex 里多一个 `$zloop` |
| `--claude-stop-hook` | 往 `~/.claude/settings.json` 的 `hooks.Stop` 加一条 `zloop hook-stop`（实验性，默认不装） |
| `--sudoers` | macOS：写 `/etc/sudoers.d/zloop-pmset`，让 runner 能关掉合盖休眠（会要一次密码） |

**新开的**会话才会加载新 skill。

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

### `next` 怎么决定

```
paused/done  >  all_done  >  user_gate / blocked  >  fail_streak  >  progress_streak  >  throttled  >  ready
```

- 有可执行 todo（`open` 且 `blocked_by` 全部 done）→ `ready`，选 `(priority, 写入顺序)` 最靠前的一条，`interval_min = 3`。
- 全部 blocked 且有人在等 → `user_gate`；纯依赖未满足 → `blocked`。退避 10 → 30 分钟，交互式连续 3 次 noop 后 `interval_min = null`。
- 最近连续 3 次 `fail` → `fail_streak`；同一 todo 连续 8 次 `progress` → `progress_streak`；两者都停下等人，`edit` 重置。
- 24 小时窗口内记账满 `max_runs` → `throttled`，给出几分钟后释放。

### `.zloop/` 目录与状态文件

```
.zloop/
  state.json            当前目标的唯一真源（下面的结构）
  state.json.lock       并发锁（flock）
  goals/<id>.json       停放着的其他目标，结构和 state.json 一样（zloop goal switch 换车位）
  NOTES.md              zloop remember 写的经验（项目共享，不跟着目标走）
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
                "host": "claude", "session": "36346c2a-…", "log": "log/20260827-055458-t1-done.md",
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
| 源码文件 / 行数 | 819 / 317,699（Python） | 19 / ≈4921（Rust） |
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
| 运行时依赖 | 0 | 0（静态单二进制） |

### 明确不做

多 agent 协作、能力路由、仪表盘、聊天、飞书、事件溯源、全局跨项目 registry、PreToolUse 强制拦截、开机自启。两个 agent 同时跑同一个 `.zloop/` 不会写坏文件，但会互相覆盖进度——这是有意为之。

### 开发

```bash
cargo test                       # 71 个用例（tick / todo / state / cli / runner，runner 用假宿主，约 2 分钟）
cargo build --release && install -m755 target/release/zloop ~/.local/bin/zloop
```

目录：`src/` 实现 · `tests/` 集成测试 · `docs/`：`RUST-DESIGN.md` 当前设计、`LONG-RUN-AUDIT.md` 长程加固审计、`OPEN-SOURCE-REVIEW.md` 开源方案对照与借鉴、`TEST-REPORT.md` 自测报告、`loopx-principles.md` / `loopx-scheduling-notes.md` loopx 研究、`DESIGN.md` v0.1 Python 原型设计记录。

MIT License.
