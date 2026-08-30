# zloop

**让 Claude Code / Codex 围着一个目标持续干活的最小调度器。**

一个 JSON 状态文件、一个 1.6 MB 的静态 Rust 二进制，不需要任何解释器、服务或后台守护。
你给它一个目标和几条 todo，它一轮做一条、做完写回、该停就停、能接着跑就接着跑——跑多久都行。

## 解决什么问题

宿主自带的 `/goal`、`/loop until:` 是**单个会话内**的循环：关掉窗口就没了，换个宿主要重讲一遍，
做过什么只留在聊天记录里。zloop 的价值全在会话之外：

| 问题 | zloop 怎么办 |
|---|---|
| **关掉终端任务就断** | `zloop start` 起后台 runner 驱动 `claude -p` / `codex exec`，一轮一条 todo。宿主挂死会超时收掉、限流会退避、等你决定时慢速轮询不退出；进程被杀再 `start` 一次就续上 |
| **Claude Code ↔ Codex 之间切换要重讲一遍** | 两个宿主读同一个 `.zloop/state.json`；`zloop context` 给一份 ≤4000 字符的交接包（目标 / 当前判断 / 下一条 / 待办 / 各宿主会话） |
| **做过什么只留在聊天记录里** | 每轮生成一份分节技术文档：实现思路、关键决策、遇到的坑、验证证据、自动抓取的改动文件 |
| **想回到当时的会话看细节** | 每轮自动记下会话 id，`zloop sessions` 直接给你 `claude --resume <id>` / `codex resume <id>` |
| **它卡住了你不知道** | 进入等人、限流、停机时推一条飞书或任意命令通知 |
| **计划做到一半发现方向错了** | 每条 todo 写回后做一次纯代码体检，命中信号才提议重排；开 `--auto-replan` 可以自己换路线，护栏在代码里强制 |

跑多久都行不是说法：2026-08-29 有过一次 4 小时 17 轮、窗口内 0 次人工介入、产出 16 个真提交的无头运行，
判据取自 zloop 之外的证据（见 [长程运行实证](docs/audit/LONG-RUN-PROOF.md)）。

## 目录

- [一、安装](#一安装)
- [二、使用](#二使用)
- [三、深入](#三深入)

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
zloop --version                              # zloop 0.4.0
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

下面按**怎么用**组织：先讲清五个概念，再讲四种驱动方式，然后是看进度、停机处理、调参。**想查某条命令干什么、什么时候敲、有哪些参数，直接看[命令详解](docs/guide/COMMANDS.md)。**

### 1. 五个概念

| 概念 | 是什么 | 在哪 |
|---|---|---|
| **goal** | 一个项目**当前**的目标，一句话；其余目标停在 `.zloop/goals/` | `.zloop/state.json` → `goal.text`；换目标用 `zloop goal new` / `goal switch`（见 [一个项目多个目标](docs/guide/MULTI-GOAL.md)） |
| **todo** | 目标拆成的有序步骤，带 `[P0]/[P1]/[P2]` 优先级；状态 open / blocked / deferred / done | `zloop plan` 写入，`zloop done` / `zloop edit` 改 |
| **outcome** | 一条 tick 的结果。**干活的**：`done` / `progress` / `fail`（只有这三种算轮次、算配额）。**其余**：`block`（等人）、`noop`（该轮没跑）、`edit`（改了 todo）、[`feedback`](docs/guide/COMMANDS.md#zloop-feedback-todo-人说的)（**你**说的话）、[`reflect`](docs/guide/COMMANDS.md#zloop-reflect)（整理经验）、[`replan`](docs/guide/COMMANDS.md#zloop-replan)（重估计划） | `ticks[].outcome` |
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
| `⏳ 等 t3` | 在等前置 todo，等哪条直接写出来。t3 还活着（`open` / `blocked`），迟早轮得到 |
| `⛔ 等不到 t3` | **永远轮不到**：t3 已延后、状态被手改成 zloop 不认的词、或压根不在清单里（手改过 `state.json`；`compact` 不会造出这种，它会把还有人等的那条留下）。依赖要 `done` 才放行，而它已经派不出去——**解开的命令就在它下面**（`↳ 解开敲 …`），和 `doctor` 的 `dead_blocked_by` / `dangling_blocked_by` 是同一条判据。多条依赖只要有一条这样就算，不管它排第几 |
| `○ 排队中` | 排在后面，没被挡 |
| `⏭ 已延后` | `zloop edit t6 --status deferred` 挂起的。**不算进百分比的分母**——调度器把它当已了结，所以进度写成 `6/6 完成 · 2 条延后`，而不是 6/8 |

超过 15 步就折叠：没做完的全留着，前面垫 3 步做过的当上下文，其余收成 `… 前 11 步已收起`。

**③ 明细 + 页脚 = 我该敲什么。** 灰标签是情况，青标签是**可以直接抄走的命令**，一行一条：

- `目标` / `清单` / `阶段` / `后台` **永远在**；`其他`（还有几个目标停着）/ `睡眠` / `文档` / `会话` 只在有话要说时出现（睡眠设置正常、没有缺文档的轮次就不占行）；
- 页脚随状态变：就绪 → `开跑`；执行中 → `写回 zloop done t2 --note … --approach …`；等你决定 → 命令已贴在那一步下面；限流中 → `放宽`；已停 → `看失败` + `重跑`；已暂停 → `继续`；完成 → `加活` / `换目标` / `出文档`。

八种状态各自长什么样、页脚给哪几条命令，见 [`status` 八种状态实拍](docs/guide/STATUS-GALLERY.md)。

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

### 7. 停下来了怎么办

下表左列是 `zloop context` / `zloop next --json` 里的 `phase` 字段（`status` 显示的是它的压缩版）：

| `phase` / runner 输出 | 发生了什么 | 你要做的 |
|---|---|---|
| `stopped (done)` | 全部 todo 完成 | 同一件事继续做：`zloop plan --add …` 加活会自动回到 active；**换一件事**：`zloop goal new "新目标"`（旧目标原地停放，[一个项目多个目标](docs/guide/MULTI-GOAL.md)） |
| `waiting (user_gate)` / runner 日志 `polling until a human unblocks` | 某条 todo 被 `--block` 等你决定；后台 runner **没有退出**，在 30 分钟一次慢速轮询 | `zloop status` 看 `!` 那条下面 `↳` 的问题 → 回答后 `zloop edit t3 --status open`（必要时 `--text` 改写），runner 下次轮询自动续 |
| `stopped (fail_streak)` | 连续 3 轮失败：宿主超时、没写回、或真的报错 | `zloop log --todo t3` 看原因 → 修环境 / 拆 todo / `zloop edit t3 --text …`（**停下之后**任意 `edit` 都重置计数；循环还在跑的时候只有改**正在失败的那条** todo 才算，见下）→ `zloop start` |
| `stopped (progress_streak)` | 同一 todo 连续 8 轮"有进展"却没完成 | 多半 todo 太大：`zloop edit t3 --text "更小的一步"` 或 `zloop done t3 --outcome progress --next "拆出的下一步"` |
| `stopped (paused)` | 你 `zloop pause` 了 | `zloop resume` |
| `stopped (budget)` | 累计花费达到 policy `max_total_usd` | 看 `zloop status` 标题行的 `$花费/上限`，确认值得就调大上限再 `start` |
| `waiting (throttled) · retry in N min` | 24 小时内已跑满 `max_runs`（默认 480） | 确实需要更快就调大 policy 的 `max_runs`，或设 `0` 不限 |
| runner 日志 `host rate-limited · not counted · sleeping 30 min` | 宿主返回 429 / rate limit / overloaded | 不用管，30 分钟后自动重试，不计失败 |
| runner 日志 `TIMED OUT (recorded fail)` | 某轮宿主超过 `--timeout-min` 被 kill | 偶发不用管；频繁出现就调大 `--timeout-min` 或把 todo 拆小 |
| `start: 没启动——runner 起来第一轮就会退出（…）` | `start` 的启动前体检：这一轮起来也是秒退，所以没起（退出码 1） | 照它第二、三行说的做（0 待办就 `zloop plan`，做完了就 `zloop goal new`，等等），再 `start` |

---

## 三、深入

README 只讲「干什么、怎么装、怎么用」。其余都在 [`docs/`](docs/)：

### 用法细节 · [`docs/guide/`](docs/guide/)

| 文档 | 讲什么 |
|---|---|
| [命令详解](docs/guide/COMMANDS.md) | 每条命令干什么、什么时候敲、参数和实测例子 |
| [`status` 八种状态实拍](docs/guide/STATUS-GALLERY.md) | 每种状态屏幕上长什么样、页脚给的是哪几条命令 |
| [每条 todo 留一份技术文档](docs/guide/TECH-DOCS.md) | `--approach` / `--decision` / `--pitfall` / `--evidence` 写进 `.zloop/log/` |
| [一个项目多个目标](docs/guide/MULTI-GOAL.md) | 停放、切换、归档，以及「只看得见当前项目」这条边界 |
| [调参](docs/guide/TUNING.md) | `policy` 七个字段 + runner 参数各控制什么 |
| [内部：`next` 怎么决定](docs/guide/INTERNALS.md) | 调度决策梯，`.zloop/` 目录与 `state.json` 结构 |
| [与 loopx 的对比](docs/guide/VS-LOOPX.md) | 为什么是这个规模，哪些功能是有意不做的 |
| [从 loopx 迁移](docs/guide/MIGRATE-FROM-LOOPX.md) | 把 loopx 的状态导进来 |
| [开发](docs/guide/DEVELOPMENT.md) | 构建、测试、格式闸，以及写文档的约定 |

### 设计与取舍 · [`docs/design/`](docs/design/)

为什么这么做，以及借鉴了谁。[整体设计](docs/design/DESIGN.md) ·
[Rust 重写的取舍](docs/design/RUST-DESIGN.md) ·
[自适应重规划](docs/design/ADAPTIVE-REPLAN.md) ·
[自我改进回路](docs/design/SELF-IMPROVEMENT.md) ·
[合盖不休眠](docs/design/KEEP-AWAKE.md) ·
[loopx 的原则](docs/design/loopx-principles.md) ·
[loopx 的调度笔记](docs/design/loopx-scheduling-notes.md)

### 审查与实证 · [`docs/audit/`](docs/audit/)

做过什么检查、查实了什么。[42 条确认缺陷清册](docs/audit/FINDINGS.md) ·
[全量代码审查](docs/audit/CODE-AUDIT.md) ·
[长程运行实证](docs/audit/LONG-RUN-PROOF.md) ·
[长程加固审查](docs/audit/LONG-RUN-AUDIT.md) ·
[多目标模块审查](docs/audit/GOALS-REVIEW.md) ·
[status 界面审查](docs/audit/STATUS-REVIEW.md) ·
[测试报告](docs/audit/TEST-REPORT.md) ·
[开源方案调研](docs/audit/OPEN-SOURCE-REVIEW.md)

---

zloop 是对 [loopx](https://github.com/huangruiteng/loopx) 里「Claude Code / Codex 核心调度」那 20% 的重写：
保留「状态 → 该不该跑 → 跑一条 → 写回 → 决定下一 tick」这条主干，砍掉多 agent、能力插件、仪表盘、
30 种交互模式和 32 万行代码。MIT 许可。
