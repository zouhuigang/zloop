# 确认缺陷清册（42 条）

> **这份是索引，正文在 [`docs/CODE-AUDIT.md`](CODE-AUDIT.md)。**
> 那边按审查轮次排，22 节三千多行；这边一条缺陷一段，每条都带锚点直达正文的那一小节。
> 它原来是 CODE-AUDIT 正文最后的一节——想查一条缺陷得先滚过 2800 行，所以 t45 把它抽了出来。
>
> 抽的时候顺带修掉了正文里三处腐烂的引用（都是「写歪了也没人报错」的那种）：
> ① **有两节的编号都是 6**（第三轮和第四轮），十一处指向「第 6 节」的引用有一半落错地方——已重编号，
> 现在 §N ＝ 第 (N−3) 轮；② 开头两处「见第 2 节的 A-1 / B-1」其实该指
> [§4 发现清单](CODE-AUDIT.md#4-发现清单)；③ 正文里一条已有的锚点链接本身就是坏的
> （`#a-16…next-就能…`，标题里根本没有那个连字符）。
> 从此这类腐烂有闸：`scripts/check-doc-links.py` 已并进 `sh scripts/check.sh` 和 CI。

## 1. 口径：什么进这张表

这个目标从第一天起就说了「不存在证不出来」，所以这张表的准入线只有一条：
**有可复现的失败场景**——一个脚本、一条回归测试，或者一段逐字记下来的命令输出。

- **进表**：[§2 一览](#2-一览) / [§3 逐条草稿](#3-逐条草稿) 的 42 条。每条都能指出"敲什么会看到什么"。
- **不进表，但记一笔**：[§4 不修](#4-不修查过确认不是缺陷)——查过、确认不是缺陷（有意设计 / 边界都在 / 场景太构造）。
- **不进表，因为没复现**：[§5 试过但没复现](#5-试过但没复现不进-issue)——试过、没做出来。写下来是为了让下一个人别重复试，
  **不是**为了凑数；混进 issue 里会污染真发现。

**为什么是"草稿"不是 issue**：项目约定——本次审查只 `git commit` 到本地，
不 `git push`、不开 GitHub issue（修复还没被人看过）。这一节就是等人看的那份东西：
一条一条可以直接复制成 issue 正文，也可以直接当作"已经修完了什么"的验收清单。
真要开的时候，仓库里的 `scripts/gh-issues.py` 负责 issue ↔ todo 的绑定（约定是
todo 文本结尾带 `(#N)`）。

**严重度口径**：高 = 会丢活/丢钱/长跑失控且没人被通知；中高 = 同样的后果但要多一个前提；
中 = 状态或读数出错、有出口但难发现；低 = 绊子、脏东西、说法不准。

## 2. 一览

| # | id | 严重度 | 一句话 | 状态 | 修复 commit | 正文 |
|---|---|---|---|---|---|---|
| 1 | A-1 | 高 | `install --claude-stop-hook` 撞上形状不对的 settings.json 就 panic | 已修 | `c917421` | [§4](CODE-AUDIT.md#a-1高zloop-install---claude-stop-hook-在-settingsjson-结构不对时-panic--已修) |
| 2 | A-2 | 中 | `remember --rule` 读-改-写无锁，并发丢条目 | 已修 | `d07d003` | [§5](CODE-AUDIT.md#a-2中zloop-remember---rule-并发会丢条目--已修t6commit-d07d003) |
| 3 | A-3 | 低 | 残留 `state.json.tmp` 没人清，`doctor` 也不说 | 已修 | `d07d003` | [§5](CODE-AUDIT.md#a-3低被杀之后留下-statejsontmpdoctor-说没发现问题--已修t6commit-d07d003) |
| 4 | A-4 | 高 | NOTES.md 一个非 UTF-8 字节 → 约定和经验静默清零并被真删掉 | 已修 | `36b0e3c` `90f2c5d` | [§6](CODE-AUDIT.md#a-4高notesmd-里一个非-utf-8-字节--约定和经验静默清零下一次-remember---rule-把它们真删掉--已修) |
| 5 | A-5 | 高 | `--exit-on-wait` 在「等人」时从不生效（实测空转 20 小时） | 已修 | `8131c7c` | [§6](CODE-AUDIT.md#a-5高--exit-on-wait-在等人时从不生效它只在一种-runner-自己走不到的状态下才管用--已修) |
| 6 | A-6 | 高 | 超时管不住留下孙进程的那一轮，期间 SIGTERM 叫不动 | 已修 | `ca2074b` | [§6](CODE-AUDIT.md#a-6高超时管不住留下后台进程的那一轮而且这段时间里-sigterm-叫不动-runner--已修) |
| 7 | A-7 | 中 | `policy.window_hours` 越界 → `next`/`status`/`context` 全 panic | 已修 | `c917421` `1e6f103` | [§6](CODE-AUDIT.md#a-7中policywindow_hours-手滑一下next--status--context-全-panic而-doctor-说没问题--已修)、[§6 复核](CODE-AUDIT.md#a-7-复核t28两处钳位分属两条分支cli-面只盖住了一条) |
| 8 | A-8 | 中 | 时间参数「装得下 i64」就 panic，装不下反而有好提示 | 已修 | `c917421` | [§6](CODE-AUDIT.md#a-8中时间参数装得下-i64就-panic装不下反而有好错误提示--已修) |
| 9 | A-9 | 中高 | 依赖成环没人拦，永久卡死且 `doctor` 无诊断 | 已修 | `ba87ca2` | [§7](CODE-AUDIT.md#a-9中高依赖成环没人拦永久卡死且无诊断--已修) |
| 10 | A-10 | 中 | 「0 = 关掉这个检查」只对五个阈值里的三个成立 | 已修 | `ba87ca2` | [§7](CODE-AUDIT.md#a-10中0--关掉这个检查只对三个阈值成立--已修) |
| 11 | A-11 | 高 | 时钟跳到未来 + 撞配额 = runner 睡 72 年，面板一切正常 | 已修 | `0ff7fe0` | [§7](CODE-AUDIT.md#a-11高时钟跳到未来--撞配额--runner-睡-72-年而-status-看着一切正常--已修) |
| 12 | A-12 | 高 | `--git-commit` 的 checkpoint 提交整个工作树 | 已修 | `3c1865b` | [§8](CODE-AUDIT.md#a-12高--git-commit-的-checkpoint-提交整个工作树--已修) |
| 13 | A-13 | 高 | 基线只拍一次，长跑中途邻居新建的文件都算我们的 | 已修 | `15106bf` | [§8](CODE-AUDIT.md#a-13高快照只拍一次长跑中途冒出来的都算我们的--已修) |
| 14 | A-14 | 高 | git 一挂住 runner 跟着挂住，SIGTERM 叫不动 | 已修 | `7d18b26` | [§9](CODE-AUDIT.md#a-14高git-一挂住runner-跟着挂住而且-sigterm-叫不动它--已修) |
| 15 | A-15 | 中高 | 写回路上的裸 git 挂住 → 整轮的账和技术文档不落盘 | 已修 | `7384774` | [§9](CODE-AUDIT.md#a-15中高写回路上的裸-git-挂住--这一轮的账和技术文档一个字都没落盘--已修) |
| 16 | A-16 | 中高 | 人敲三下 `zloop next`，长跑就拒绝启动（noop 计数跨进程串味） | 已修 | `1811382` | [§10](CODE-AUDIT.md#a-16中高noop-计数从交互式命令串进-runner-的停机判断人敲三下-zloop-next就能让长跑拒绝启动--已修) |
| 17 | A-17 | 高 | 人插一句 `feedback`，失败的一轮被记成「写回了」 | 已修 | `6ff3793` | [§11](CODE-AUDIT.md#a-17高人插一句-zloop-feedback一轮失败的宿主就被记成写回了连续失败停机整个失效--已修) |
| 18 | A-18 | 中 | `compact` 把花费一起归档走，`max_total_usd` 静默复位 | 已修 | `73c16cb` | [§11](CODE-AUDIT.md#a-18中zloop-compact-把花费一起归档走max_total_usd-静默复位--已修) |
| 19 | A-19 | 中高 | 人留一句反馈，下一轮无头 runner 就 `--resume` 进人的对话 | 已修 | `4dd8499` | [§11](CODE-AUDIT.md#a-19中高人留一句反馈下一轮无头-runner-就---resume-进人的对话里--已修) |
| 20 | A-20 | 高 | 改**别的** todo 就把连续失败停机这道闸拆了 | 已修 | `73740b7` | [§12](CODE-AUDIT.md#a-20高人顺手整理-backlogzloop-edit-改别的-todo连续失败停机这道闸就被拆了--已修) |
| 21 | A-21 | 高 | 一句 `feedback` 让「原地踏步」那道闸永远数不到上限 | 已修 | `73740b7` | [§12](CODE-AUDIT.md#a-21高人插一句-zloop-feedback同一条-todo-原地踏步那道闸就永远数不到上限--已修) |
| 22 | A-22 | 中高 | 依赖一条已延后的 todo：一样永远等不到，`doctor` 却退 0 | 已修 | `cf29c2b` | [§13](CODE-AUDIT.md#a-22中高依赖一条已延后的-todo卡死的形状一模一样doctor-却退-0--已修) |
| 23 | B-1 | 低 | `Decision` 的 "should_run ⇒ todo 非空" 没人守（绊子，非 bug） | 已修 | 本轮（t10） | [§4](CODE-AUDIT.md#b-1低decision-的-should_run--todo-非空-不变量没人守--已修t10) |
| 24 | B-2 | 低 | `edit <id> --blocked-by <它自己>` 被收下，那条 todo 永久卡死 | 已修 | `ba87ca2` | [§6](CODE-AUDIT.md#b-2低edit-id---blocked-by-它自己-被收下那条-todo-就再也跑不了--已修t12commit-ba87ca2) |
| 25 | B-3 | 中 | 全部 deferred 时说「目标结束」，引着人去开新目标 | 已修 | `e768b5d` | [§7](CODE-AUDIT.md#b-3重估为中全部-deferred-时说目标结束并引着人去开新目标--已修) |
| 26 | T21 | 中 | `awake.rs` 8 处裸子进程（收口 5 处，留 3 处并写明理由） | 已修 | `7941e6d` | [§19](CODE-AUDIT.md#19-第十六轮t21中awakers-的-8-处裸子进程--收口-5-处留-3-处并写明理由) |
| 27 | T29 | 中 | `compact` 把「跑了几轮」一起搬走，四处读数清零 | 已修 | `6e85fcc` | [§21](CODE-AUDIT.md#t29中一次例行整理把-status--stats--replan--轮次编号四处读数一起清零--已修) |
| 28 | T30 | 中 | 格式闸全红 = 没有闸（缺 `rustfmt.toml`） | 已修 | `ac040d3` `ffb6f59` | [§2.1](CODE-AUDIT.md#21-格式闸原先是空的已修t30) |
| 29 | T31 | 中 | 闸有定义但没人自动按（没有 CI） | 已修 | `789d524` | [§2.2](CODE-AUDIT.md#22-闸有了定义但没人自动去按已修t31) |
| 30 | T32 | 中 | `policy.intervals_min` 越界：debug 崩、release 睡 8171 年 | 已修 | `dc7e714` | — |
| 31 | T33 | 低 | 阶梯写反 `[30,10,3]` 三件事一起反过来，`doctor` 沉默 | 已修 | `7dc358e` | — |
| 32 | T34 | 低 | `slowest_interval` 用 `.last()` 取「最慢的一档」，阶梯非单调时拿的不是最大值 | **未修（待定）** | — | — |
| 33 | T36-① | 低 | `tests/scratch_t33.rs` 被误提交（文件里写着"未被 git 跟踪"） | 已清 | `503e427` | [§14](CODE-AUDIT.md#t36-低testsscratch_t33rs-被误提交进仓库--已清) |
| 34 | T36-② | 中 | `status` 对「永远等不到」和「正常排队」用同一个词 | 已修 | `503e427` | [§14](CODE-AUDIT.md#t36-中status-对永远等不到和正常排队用同一个词--已修) |
| 35 | T37 | 中 | 「永远等不到」只在 `status` 一块屏上说了（另三处清单还在骗人） | 已修 | `bca786a` | [§14](CODE-AUDIT.md#t37中永远等不到只在-status-一块屏上说了--已修并补上-t36-漏判的那一半) |
| 36 | T38 | 中 | 延后一条依赖 = 一条命令判死一片，回显只字不提 | 已修 | `0c27d16` | — |
| 37 | T39 | 中高 | `compact` 搬走还有人依赖的那条，等它的几条永远等不到 | 已修 | `c441398` | [§15](CODE-AUDIT.md#t39中高compact-把还有人依赖的那条搬进归档等它的那几条就此永远等不到--已修) |
| 38 | T40-① | 中高 | 例行 `compact` 吃掉人今天刚留下、还没人读过的反馈 | 已修 | `331293a` `5304382` | [§16.2](CODE-AUDIT.md#162-t40-中高例行-compact-吃掉人今天刚留下的还没人读过的反馈--已修见-17)、[§17](CODE-AUDIT.md#17-第十四轮compact-剩下的两处指针一起收口t40--已修) |
| 39 | T40-② | 中 | `compact --force` 搬走在飞的那条，`ensure_idle` 的两条出口都退 2 | 已修 | `331293a` `5304382` | [§16.3](CODE-AUDIT.md#163-t40-中compact---force-把在飞的那条搬走ensure_idle-给的两条出口从此都退-2--已修见-17)、[§17](CODE-AUDIT.md#17-第十四轮compact-剩下的两处指针一起收口t40--已修) |
| 40 | T42 | 中高 | 派活指着一条已了结的 todo 时，四处出口一起坏 | 已修 | `2e0168f` | [§18](CODE-AUDIT.md#18-第十五轮t42中高派活指着一条已了结的-todo-时四处出口一起坏--已修) |
| 41 | T43 | 中 | `install_sudoers` 的暂存路径别人占得住名字 | 已修 | `b406ded` | [§20](CODE-AUDIT.md#t43中install_sudoers-的暂存路径别人也能占名装进-etcsudoersd-的可以不是我们写的那份--已修) |
| 42 | T44 | 中 | 整理一次账本，进度条 66% → 0%、「一次过 2/2」→「0/0」 | 已修 | `71e5c8a` | [§22](CODE-AUDIT.md#t44中整理一次账本进度条-66--0一次过-2200--已修) |

**41 已修（含 T36-① 的"已清"）/ 1 未修（T34，已排成下一条待办）/ 0 不修。**
分布：高 11、中高 8、中 17、低 6。

## 3. 逐条草稿

每条五行：复现怎么做、修之前看到什么、什么算修好了、现在是什么状态和为什么、正文在哪。
**正文那一项是锚点链接**，点进去落在 CODE-AUDIT 的那一小节上，不用自己去数节号。

#### A-1（高）· 已修 `c917421`
- **标题**：`zloop install --claude-stop-hook`：`settings.json` 是合法 JSON 但结构不对时 panic
- **复现**：`HOME` 指向临时目录，写入 `{"hooks": []}`，跑 `zloop install --claude-stop-hook`
- **修复前**：panic `hosts.rs:257`。7 种形状（`[]`/`"hello"`/`42`/`{"hooks":[]}`/`{"hooks":"none"}`/`{"hooks":{"Stop":{}}}`/`{"hooks":{"Stop":"off"}}`）全崩；反倒是**语法坏**的 `{oops` 处理得很好（exit 2 + 说明）
- **验收**：7 种形状全部 exit 2 + 说明；逐字节比对磁盘内容**没变**；`{}`/`{"hooks":{}}`/`{"hooks":{"Stop":[]}}` 照旧写得进去
- **状态**：已修 · 回归测试 `cli_test::a_wrongly_shaped_settings_json_is_reported_not_panicked_or_clobbered`（三层分别换回 `.expect()` 都变红）· 正文：[§4 · A-1](CODE-AUDIT.md#a-1高zloop-install---claude-stop-hook-在-settingsjson-结构不对时-panic--已修)

#### A-2（中）· 已修 `d07d003`
- **标题**：`zloop remember --rule` 是无锁的读-改-写，并发会丢条目
- **复现**：20 个进程同时 `zloop remember --rule "…"`；再试 10 追加 + 10 `--rule` 混合
- **修复前**：20 → **12** 落地；混合场景 20 → **16**（`--rule` 的整文件重写吞掉了它读入之后别人追加的经验）。文件不损坏，丢的是条目，而且悄无声息
- **验收**：两个场景都 20/20；纯追加那条路照旧不用等锁
- **状态**：已修 · `notes.rs` 的读-改-写走 `state::locked`（NOTES 自己一把锁，见 `lock_target`）· 正文：[§5 第二轮 · A-2](CODE-AUDIT.md#a-2中zloop-remember---rule-并发会丢条目--已修t6commit-d07d003)

#### A-3（低）· 已修 `d07d003`
- **标题**：被杀之后留下的 `state.json.tmp` 没人清，`doctor` 说"没发现问题"
- **复现**：手工在 `.zloop/` 放一个 `state.json.tmp`，跑 `zloop doctor`
- **修复前**：exit 0「没发现问题」。文件永远不会被清理，攒着占地方
- **验收**：`doctor` 报 `leftover_tmp`，说清"正本没事、这些可以删"，并把 `<x>.tmp.<pid>`（持有者记录用的那种）也认出来
- **状态**：已修 · `doctor.rs::check_leftover_tmp` · 正文：[§5 第二轮 · A-3](CODE-AUDIT.md#a-3低被杀之后留下-statejsontmpdoctor-说没发现问题--已修t6commit-d07d003)

#### A-4（高）· 已修 `36b0e3c` `90f2c5d`
- **标题**：NOTES.md 里一个非 UTF-8 字节 → 约定和经验静默清零，下一次 `remember --rule` 把它们真删掉
- **复现**：正常的 NOTES.md（2 约定 / 3 经验）后面追加一个 `\xff`，跑 `zloop context`，再敲一次 `zloop remember --rule`
- **修复前**：第二步 `context` 带的护栏变成 **0 / 0**（文件里其实都还在），第三步之后磁盘上只剩 1 约定 / 0 经验、**没有备份**；全程 `doctor` 都说没问题
- **验收**：读不出来就**拒绝写回**（不拿空的盖掉），`context` 不静默交出护栏，`doctor` 出声
- **状态**：已修（两个 commit：写侧 + 体检侧）· 正文：[§6 第三轮 · A-4](CODE-AUDIT.md#a-4高notesmd-里一个非-utf-8-字节--约定和经验静默清零下一次-remember---rule-把它们真删掉--已修)

#### A-5（高）· 已修 `8131c7c`
- **标题**：`--exit-on-wait` 在「等人」时从不生效——它只在一种 runner 自己走不到的状态下才管用
- **复现**：`sh scripts/repro-a5-exit-on-wait.sh`（全程真实路径：宿主自己 `done --block` 把活交回给人，下一轮 runner 撞 `user_gate`）
- **修复前**：脚本退 1——15 秒后进程还在，journal 2 条 `sleep`、账本 **0 条 noop**。审的时候机器上抓到现行：一个带 `--exit-on-wait` 的 runner 在 `user_gate` 上转了 **20 小时 24 分**，写了 1849 条 journal 并一路占着 keep-awake
- **验收**：脚本退 0；`exit_on_wait` 由**标志**说了算而不是由 noop 计数说了算；继续等的那一支打印 `… (polling until a human unblocks)`
- **状态**：已修 · 回归测试 `runner_test::exit_on_wait_stops_the_first_time_the_runner_itself_hits_a_human_gate` + 新增 `run_within()` 助手（这类回归撤掉修复是**挂住**而不是变红，挂住的测试没人当成失败）· 正文：[§6 第三轮 · A-5](CODE-AUDIT.md#a-5高--exit-on-wait-在等人时从不生效它只在一种-runner-自己走不到的状态下才管用--已修)

#### A-6（高）· 已修 `ca2074b`
- **标题**：超时管不住留下孙进程的那一轮，而且这段时间里 SIGTERM 叫不动 runner
- **复现**：`policy.preflight_cmd = "sleep 20 &"`，`zloop run --timeout-min 3 --fast`（3 秒上限）
- **修复前**：runner 实际耗时 **21 秒**——耗时跟着孙进程寿命走，跟 `--timeout-min` 没关系（`read_to_string` 在等管道 EOF，而孙进程继承了同一个写端）。宿主那条路一模一样
- **验收**：超时那一轮连孙进程一起收（进程组），排水有上限；这段时间里 SIGTERM 叫得动
- **状态**：已修 · 正文：[§6 第三轮 · A-6](CODE-AUDIT.md#a-6高超时管不住留下后台进程的那一轮而且这段时间里-sigterm-叫不动-runner--已修)

#### A-7（中）· 已修 `c917421`（+ `1e6f103` 复核补第二处）
- **标题**：`policy.window_hours` 手滑一下，`next` / `status` / `context` 全 panic，而 `doctor` 说没问题
- **复现**：把 `.zloop/state.json` 的 `policy.window_hours` 改成越界值，跑 `zloop next`
- **修复前**：panic（`tick.rs:186` 的 `at - Duration::hours(...)` 没有范围检查）。这不是"内部文件"——zloop 自己就在教人去改隔壁的 `policy.max_total_usd`
- **验收**：钳进 `0..=8760`，三条命令都不崩；`doctor` 报 `bad_policy`
- **状态**：已修 · 回归测试 `an_out_of_range_window_hours_gets_clamped_instead_of_panicking` 等；**t28 复核发现两处钳位分属两条分支、CLI 面只盖住了一条**，`1e6f103` 补上第二处的覆盖 · 正文：[§6 第三轮 · A-7](CODE-AUDIT.md#a-7中policywindow_hours-手滑一下next--status--context-全-panic而-doctor-说没问题--已修)、[§6 第三轮 · A-7 复核（t28）](CODE-AUDIT.md#a-7-复核t28两处钳位分属两条分支cli-面只盖住了一条)

#### A-8（中）· 已修 `c917421`
- **标题**：时间参数「装得下 i64」就 panic，装不下反而有好错误提示
- **复现**：`zloop compact --keep-days 99999999999`
- **修复前**：panic `cli.rs:1990`；再大一位（`999999999999999`）反而是 chrono 的友好错误。两个入口同一个根因（`now() - Duration::…(n)` 无 checked）
- **验收**：5 个越界取值全变 exit 2 + 人话；`--since 7d` / `--keep-days 30` 一条没被误伤；**验参挪到 `ensure_idle` 之前**（连范围都不对的参数不该先去抢闸）
- **状态**：已修 · 回归测试 `out_of_range_time_arguments_get_the_same_friendly_error_as_garbage` · 正文：[§6 第三轮 · A-8](CODE-AUDIT.md#a-8中时间参数装得下-i64就-panic装不下反而有好错误提示--已修)

#### A-9（中高）· 已修 `ba87ca2`
- **标题**：依赖成环没人拦，永久卡死且无诊断
- **复现**：`zloop edit t1 --blocked-by t2` + `zloop edit t2 --blocked-by t1`（二元环，两条都是真命令）
- **修复前**：`next` 一路 `blocked` + 「隔一阵重试」，`status` 说「等依赖 · 30 分钟后重试」，`doctor` 说「没发现问题」
- **验收**：`doctor` 报环并说清怎么解开；`edit` 挡住唯一一个不用看全图就能判定的环（自依赖，见 B-2）
- **状态**：已修 · 回归测试三条（`doctor_test.rs`，二元环用**真命令**造）· 正文：[§7 第四轮 · A-9](CODE-AUDIT.md#a-9中高依赖成环没人拦永久卡死且无诊断--已修)

#### A-10（中）· 已修 `ba87ca2`
- **标题**：「0 = 关掉这个检查」只对五个阈值里的三个成立
- **复现**：`max_fail_streak = 0`（或 `max_noop_streak = 0`），一个**一轮都没跑过**的新目标跑 `zloop next`
- **修复前**：`0 >= 0` 恒真 → 第一次 `next` 就返回 `fail_streak` + `interval=None`＝当场永久停机；`max_noop_streak=0` 则让 `exhausted` 恒真，非终态出口的「10 分钟后再看」塌成「停下等人」
- **验收**：五个阈值同一个口径，`0` 一律等于关掉
- **状态**：已修 · 回归测试 `tick_test::zero_turns_a_threshold_off_the_same_way_for_all_five` · 正文：[§7 第四轮 · A-10](CODE-AUDIT.md#a-10中0--关掉这个检查只对三个阈值成立--已修)

#### A-11（高）· 已修 `0ff7fe0`
- **标题**：时钟跳到未来 + 撞配额 = runner 睡 72 年，而 `status` 看着一切正常
- **复现**：账本里有一条落在未来的 tick（NTP 校时 / 改时区 / 虚拟机挂起恢复 / 电池耗尽都会造出来），同时撞上 `max_runs`
- **修复前**：`interval_min = 38048610`（72 年）
- **验收**：等待封顶在一个配额窗口（`window_hours`，最坏每窗口醒一次重判）；跨天的「睡到」带日期；未来时间戳由 `doctor` 的 `future_timestamp` 报出来
- **状态**：已修 · 回归测试两条（单元 `a_future_tick_cannot_stretch_the_throttle_wait_past_the_window` + 精确到分钟的 `22*60+1` 那条一起钉住）· 正文：[§7 第四轮 · A-11](CODE-AUDIT.md#a-11高时钟跳到未来--撞配额--runner-睡-72-年而-status-看着一切正常--已修)

#### A-12（高）· 已修 `3c1865b`
- **标题**：`--git-commit` 的 checkpoint 提交整个工作树（`git add -A -- .`）
- **复现**：在一个有别人在制品的仓库里跑 `zloop run --git-commit`，等一轮 checkpoint
- **修复前**：邻居没提交的改动被 `zloop <todo>: <note>` 一起提交走
- **验收**：只提交 runner 起跑之后变化的路径，别人的在制品留在原地；拆不开的那种打印出来并记账本
- **状态**：已修 · 正文：[§8 第五轮 · A-12](CODE-AUDIT.md#a-12高--git-commit-的-checkpoint-提交整个工作树--已修)

#### A-13（高）· 已修 `15106bf`
- **标题**：基线只在起跑那一刻拍一次，长跑中途冒出来的文件都算我们的
- **复现**：起跑 → 睡眠/回看/重估中间由别人新建文件 → 下一轮 checkpoint
- **修复前**：「不在基线里 ⇒ 是我们的」这条规则管的不是"这一轮"，而是"上次成功提交以来的一切"；而长跑里大部分墙上时间根本不在轮次里
- **验收**：每轮开工前重拍基线，**但只在上一轮结清时**（没写回 / add 失败那轮的产物还在树里等认领，重拍会把它们永远划给别人）；读不出工作树时**留着上一张**，不换空快照
- **状态**：已修 · 正文：[§8 第五轮 · A-13](CODE-AUDIT.md#a-13高快照只拍一次长跑中途冒出来的都算我们的--已修)

#### A-14（高）· 已修 `7d18b26`
- **标题**：git 一挂住，runner 跟着挂住，而且 SIGTERM 叫不动它
- **复现**：`sh scripts/repro-a14-git-hang.sh`
- **修复前**：runner 每轮 6 次裸 git（`.output()` 是**无限期**阻塞，既不看 `--timeout-min` 也不看 `stop_requested()`），git 挂住 runner 就挂住，只能 SIGKILL——而 SIGKILL 会把 `.git/index.lock` 留给孤儿 git 进程，仓库后续所有 git 写操作全废
- **验收**：git 和 `notify_cmd` 的子进程都有闸；挂住的那个整组收掉；SIGTERM 叫得动
- **状态**：已修 · 正文：[§9 第六轮 · A-14](CODE-AUDIT.md#a-14高git-一挂住runner-跟着挂住而且-sigterm-叫不动它--已修)

#### A-15（中高）· 已修 `7384774`
- **标题**：写回路上的裸 git 挂住 → 这一轮的账和技术文档一个字都没落盘
- **复现**：让 `zloop done` 里 `changed_files(root)` 那次 git 挂住
- **修复前**：`cli.rs:887` 的 `changed_files` 跑在 `state::transaction` **之前**，挂住时 note / approach / decision / pitfall / evidence 全在内存里，一个字节都没进 `state.json`——挂多久，这一轮的产物在磁盘上就不存在多久（不是"日志少一节"，是**整轮白干**）
- **验收**：写回路上的 git 也走带闸的 `run_capture`，但**跟着调用者的进程组走**（不能连调用它的人一起收）
- **状态**：已修 · 回归测试两条（`runner_test.rs`，各自撤掉对应修复就变红）· 正文：[§9 第六轮 · A-15](CODE-AUDIT.md#a-15中高写回路上的裸-git-挂住--这一轮的账和技术文档一个字都没落盘--已修)

#### A-16（中高）· 已修 `1811382`
- **标题**：noop 计数从交互式命令串进 runner 的停机判断——人敲三下 `zloop next`，长跑就拒绝启动
- **复现**：`sh scripts/repro-a16-noop-poke-kills-throttled-runner.sh`
- **修复前**：`noop` tick 全仓只有一个生产者（`zloop next` 非 `--peek` 且 `should_run=false` 时记），也就是**人在终端敲的**；runner 却拿它算 `exhausted` 去决定自己要不要停
- **验收**：`throttled` 不再由 noop 计数说了算；`max_noop_streak` 收回给 `zloop next` 用
- **状态**：已修 · 正文：[§10 第七轮 · A-16](CODE-AUDIT.md#a-16中高noop-计数从交互式命令串进-runner-的停机判断人敲三下-zloop-next就能让长跑拒绝启动--已修)

#### A-17（高）· 已修 `6ff3793`
- **标题**：人插一句 `zloop feedback`，一轮**失败**的宿主就被记成「写回了」，连续失败停机整个失效
- **复现**：`sh scripts/repro-a17-interactive-write-masks-a-failed-round.sh`
- **修复前**：结算问的是「这段时间账本长了没长」（`t.outcome != "noop"` 就算写回），而不是「宿主写回了没有」
- **验收**：`wrote_back` 只认真写回的四种 outcome；人在另一个终端写的 tick 不算
- **状态**：已修 · 回归测试三条，撤掉对应修复就变红 · 正文：[§11 第八轮 · A-17](CODE-AUDIT.md#a-17高人插一句-zloop-feedback一轮失败的宿主就被记成写回了连续失败停机整个失效--已修)

#### A-18（中）· 已修 `73c16cb`
- **标题**：`zloop compact` 把花费一起归档走，`max_total_usd` 静默复位
- **复现**：`sh scripts/repro-a18-compact-resets-budget-cap.sh`
- **修复前**：钱记在 tick 的 `cost_usd` 上，`compact` 把老 todo 名下的 tick 连锅端进 `archive/`，预算闸就此从头再来
- **验收**：归档只让**账本**变小，不让**账**变少（`Archived.cost_usd` 累计，`spent_total` 加回来）；整理也走 `ensure_idle`
- **状态**：已修 · 正文：[§11 第八轮 · A-18](CODE-AUDIT.md#a-18中zloop-compact-把花费一起归档走max_total_usd-静默复位--已修) ·**这条只是第一个受害者**，同族的 T29 / T44 在后面

#### A-19（中高）· 已修 `4dd8499`
- **标题**：人留一句反馈，下一轮无头 runner 就 `--resume` 进人的对话里
- **复现**：`sh scripts/repro-a19-runner-resumes-a-humans-session.sh`
- **修复前**：`pick_session` 只看 host 对不对、todo 对不对，**不看这条 tick 是谁记的**；而 `feedback` / `edit` 会把调用者的 `CLAUDE_CODE_SESSION_ID` 原样记进 tick
- **验收**：`--resume` 只接**宿主写回过**的会话
- **状态**：已修 · 正文：[§11 第八轮 · A-19](CODE-AUDIT.md#a-19中高人留一句反馈下一轮无头-runner-就---resume-进人的对话里--已修)

#### A-20（高）· 已修 `73740b7`
- **标题**：人顺手整理 backlog（`zloop edit` 改**别的** todo），连续失败停机这道闸就被拆了
- **复现**：`sh scripts/repro-a20-a21-another-terminal-disarms-the-brakes.sh`（四个场景、两对 A/B 对照，退 1 = 至少一条复现）
- **修复前**：`fails_in_a_row` 的兜底分支 `_ => n = 0` 把 `edit` 也收了进去，改一条**无关**的 todo 就把连续失败清零
- **验收**：改的是**正在失败的那条**才清零；改别的活只有在循环**已经停在 `fail_streak` 上**时才清零（那时人是在回应一个停着的循环）
- **状态**：已修 · 回归测试两条，各自撤掉对应修复就变红 · 正文：[§12 第九轮 · A-20](CODE-AUDIT.md#a-20高人顺手整理-backlogzloop-edit-改别的-todo连续失败停机这道闸就被拆了--已修)

#### A-21（高）· 已修 `73740b7`
- **标题**：人插一句 `zloop feedback`，同一条 todo 原地踏步那道闸就永远数不到上限
- **复现**：同上脚本（假宿主每轮 `done t1 --outcome progress`，`max_progress_streak=2`，实测 8 轮不停）
- **修复前**：`progress_streak` 从尾往前扫、`_ => break`，任何一条 `feedback` 都把它断掉——而 `feedback` 正是文档教人「跟正在跑的循环说话」的那条路
- **验收**：和 A-20 同一条规矩；`edit` 改的**就是这条 todo** 仍然无条件清零（README 给的出口就是 `edit t3 --text "更小的一步"`，活真的换了）
- **状态**：已修 · 正文：[§12 第九轮 · A-21](CODE-AUDIT.md#a-21高人插一句-zloop-feedback同一条-todo-原地踏步那道闸就永远数不到上限--已修)

#### A-22（中高）· 已修 `cf29c2b`
- **标题**：依赖一条已延后的 todo——卡死的形状和依赖环一模一样，`doctor` 却退 0
- **复现**：`zloop edit t2 --blocked-by t1` + `zloop edit t1 --status deferred`（两条真命令）
- **修复前**：`doctor` 的 `dep_cycle` 只报"环"，这种"依赖一条永远不会 done 的"一声不吭
- **验收**：`doctor` 把「永远等不到」的第三种形状也报出来
- **状态**：已修 · 回归测试三条（撤掉 `can_still_finish` 判据前两条立刻变红）· 正文：[§13 第十轮 · A-22](CODE-AUDIT.md#a-22中高依赖一条已延后的-todo卡死的形状一模一样doctor-却退-0--已修)

#### B-1（低）· 已修（本轮 t10）
- **标题**：`Decision` 的 "should_run ⇒ todo 非空" 不变量没人守
- **复现**：**没有**——这条从头到尾不是 bug，是绊子：四处 `unwrap()` 靠它，而它今天成立
- **修复前**：字段全 `pub`、没有构造器，任何模块都能拼出 `Decision { should_run: true, todo: None, .. }`，四处一起崩
- **验收**：① 全仓不再有 `Decision` 字面量（构造器 `ready`/`stop`/`wait`）；② 别的模块**编译不过**（私有字段 `_seal`，实测 `error: cannot construct 'Decision' with struct literal syntax due to private fields`）；③ 四处调用点不再 `unwrap()`（`ready_todo()`，runner 那处退化成"停下报原因"）；④ 一条覆盖 13 个出口的不变量测试，撤掉修复变红
- **状态**：已修 · 回归测试 `tick_test::every_decide_exit_keeps_should_run_implies_a_todo` · 正文：[§4 · B-1](CODE-AUDIT.md#b-1低decision-的-should_run--todo-非空-不变量没人守--已修t10)

#### B-2（低）· 已修 `ba87ca2`
- **标题**：`edit <id> --blocked-by <它自己>` 被收下，那条 todo 就再也跑不了
- **复现**：`zloop edit t1 --blocked-by t1`（唯一的 todo）
- **修复前**：exit 0 收下，此后 `next` 永远 `WAIT (blocked) · retry in 10 min`，`doctor` 说没问题；配合 A-5 就是一个不会退出、也不会通知任何人的 runner
- **验收**：exit 2 + 说清为什么永远满足不了；**一个字都不写进 `state.json`**；`--blocked-by t2` 照收
- **状态**：已修（修 A-9 时一起做的，t10 复核时才把状态补正——**这一轮没有改代码**）· 回归测试 `cli_test::edit_refuses_to_make_a_todo_depend_on_itself` + 手改文件那条由 `doctor_test::a_self_dependency_from_a_hand_edited_file_is_reported_but_a_finished_dep_is_not` 兜 · 正文：[§6 第三轮 · B-2](CODE-AUDIT.md#b-2低edit-id---blocked-by-它自己-被收下那条-todo-就再也跑不了--已修t12commit-ba87ca2)

#### B-3（中，第四轮记的是低）· 已修 `e768b5d`
- **标题**：全部 deferred 时说「目标结束」，并引着人去开新目标
- **复现**：把所有 todo `--status deferred`，看 `status` / `next`
- **修复前**：比记的严重——它根本走不到 `all_done` 那一支，因为在那之前 `edit` 的收尾已经把 `goal.status` 改成 `done` 了。于是「一条没做完、全推到以后」在面板上和「活干完了」长得一样，两条延后的活就此没人再看
- **验收**：`all_deferred` 有自己的 reason 和出口动作（把延后的捡回来，而不是开新目标）
- **状态**：已修 · 回归测试 `tick_test::all_deferred_is_not_all_done` · 正文：[§7 第四轮 · B-3](CODE-AUDIT.md#b-3重估为中全部-deferred-时说目标结束并引着人去开新目标--已修)

#### T21（中）· 已修 `7941e6d`
- **标题**：`awake.rs` 里 8 处裸子进程——收口 5 处，留 3 处并写明理由
- **复现**：`grep -n "Command::new" src/`，8 处逐个看谁在等它
- **修复前**：t20 收完 git 时留了一句「每个 zloop 起的子进程都走 `run_capture`」，`awake.rs` 里还有 8 处裸的
- **验收**：5 处收口；**不能收的 3 处把理由写进代码注释**（不只写在文档里）
- **状态**：已修 ·**这一轮真正的发现是反的**：无脑套 `run_capture` 会把恢复默认值弄没——`zloop stop` 的 SIGTERM 先把 `stop_requested()` 置上，收尾路径上的 `pmset` 探针要是也认叫停，会在第一次轮询里被自己杀掉，`SleepDisabled=1` 原地留着。所以 `run_capture` 多了一个显式的 `Stop` 参数 · 正文：[§19 第十六轮 · T21](CODE-AUDIT.md#19-第十六轮t21中awakers-的-8-处裸子进程--收口-5-处留-3-处并写明理由)

#### T29（中）· 已修 `6e85fcc`
- **标题**：一次例行整理，把 `status` / `stats` / `replan` / 轮次编号四处读数一起清零
- **复现**：`sh scripts/repro-t29-compact-drops-round-count.sh`（修之前退 1）
- **修复前**：`compact` 之后 `status` 的「跑了 4 轮」→ 0 轮、`stats` 整页消失、`replan` 的返工信号熄火、轮次编号 3 → 0 重号
- **验收**：归档汇总里存的是**按 outcome 分的计数**（能重算出全族），不是再加一个计数器
- **状态**：已修 ·**A-18 只补了花费那一个，同族还有 6 个原地等着**——修一类"累计量被搬走"的 bug 时先问它是不是一族 · 正文：[§21 第十八轮 · T29](CODE-AUDIT.md#t29中一次例行整理把-status--stats--replan--轮次编号四处读数一起清零--已修)

#### T30（中）· 已修 `ac040d3` `ffb6f59`
- **标题**：格式闸原先是空的——`cargo fmt --check` 在全仓 29 个文件上都不合规（790 hunk）
- **复现**：`cargo fmt --check`
- **修复前**：全红等于没有信号，格式漂移进来也没人拦。根因是配置缺席（~125 列的密排风格，仓库里没有 `rustfmt.toml`，rustfmt 按默认 `max_width=100` 判）
- **验收**：`rustfmt.toml` = `max_width=125` + `use_small_heuristics="Max"`（默认启发式会多出 ~2400 行凭空拆行）；对齐提交只动格式并进 `.git-blame-ignore-revs`
- **状态**：已修 · 正文：[§2.1 格式闸原先是空的（已修，t30）](CODE-AUDIT.md#21-格式闸原先是空的已修t30)

#### T31（中）· 已修 `789d524`
- **标题**：闸有了定义，但没人自动去按（仓库里没有 `.github/`）
- **复现**：翻仓库根目录
- **修复前**：t30 之后格式闸只是"人可以跑的一条命令"，不是自动拦截
- **验收**：`.github/workflows/ci.yml`（macos-latest）+ `scripts/check.sh` 一份定义（fmt → clippy `-D warnings` → test，越便宜越靠前、fail-fast），CI 和人调的是同一件事
- **状态**：已修 ·**加闸前先跑一遍**：`clippy -D warnings` 本来就有 4 条红，直接配进 CI 就是开局全红——那正是 t30 判过死刑的假闸 · 正文：[§2.2 闸有了定义，但没人自动去按（已修，t31）](CODE-AUDIT.md#22-闸有了定义但没人自动去按已修t31)

#### T32（中）· 已修 `dc7e714`
- **标题**：`policy.intervals_min` 越界 —— debug 崩、release 睡 8171 年
- **复现**：`intervals_min=[4294967295]` + 一条卡在人手里的 todo，跑 `next --peek --json` / `status` / `context`
- **修复前**：debug 构建三条命令全在 `phase.rs::human_minutes` 的 `m + 720` 上溢出 panic（exit 101）；release 不崩，`interval_min` 原样吐 4294967295 分钟（8171 年）runner 就此睡死，面板因同一处回绕印「约 0 天后重试」；`doctor --json` 的 findings 是空的——**三样东西同时说没事**
- **验收**：`clamp_interval` 把每一档钳进 `1..=7 天`（下限 1 挡的是另一头：`[0]` 会让 runner 忙等）；`human_minutes` 改 `saturating_add`；`doctor` 补取值范围（err 级）；`runner::slowest_interval` 走同一道闸
- **状态**：已修 · 回归测试 4 条，四处修复逐个撤掉都变红 ·**这是 A-7 / A-11 之后第三次重演同一个形状**（人手改的 policy 一路原样交给运行期） · 正文：**无**——正文里没有这一条，全部记录就是本条草稿（实现说明在修复 commit 里）

#### T33（低）· 已修 `7dc358e`
- **标题**：退避阶梯写反（`[30,10,3]`）时三件事一起反过来，`doctor` 沉默
- **复现**：`policy.intervals_min = [30, 10, 3]`，跑 `doctor`
- **修复前**：每一档单看都合法，所以 T32 的取值范围查不出来。实际后果：有活干的正常轮次每 30 分钟才动一次（吞吐掉到 1/10）；`blocked`/`user_gate` 退避序列变成 10→3→3（越不出活退得越快）；`slowest_interval` 拿 3 当「最慢的一档」
- **验收**：报 **warn** 不是 error（没有任何值被无声换掉，人写的数原样生效了）；只报「往回走」不报「没有严格递增」（`[10,10,10]` 是有人存心不要退避，合法）；比的是**钳过之后**的值（免得和 T32 那条 error 把同一个根因拆成两条互相矛盾的报告）
- **状态**：已修 · 正文：**无**——正文里没有这一条，全部记录就是本条草稿（实现说明在修复 commit 里）

#### T34（低）· **未修，待定**
- **标题**：`runner::slowest_interval` 用 `.last()` 取「最慢的一档」，阶梯非单调时它拿到的不是最大值
- **复现**：`intervals_min = [3, 30, 10]` → `slowest_interval` 拿到 10，真正最慢是 30
- **现状**：函数名和文档写的都是"最慢"，而「等人」「被限流」「被 host 限流」三处 sleep 都读它
- **验收（这条 todo 要产出的）**：定下是改成 `.max()` 还是维持 `.last()` 并改名，两条路各写清代价，然后照做 + 回归测试
- **状态**：**未修（待定）**——这是口径问题不是 bug：`.last()` 在阶梯单调时完全正确，改成 `.max()` 等于替用户决定"阶梯就该是升序的"。T33 的 `doctor` warn 已经覆盖了"人手写歪"的情况，剩下的是"要不要连写歪的也照着最大值睡"。已排成下一条待办（t34），不并进本轮（一轮只做一条） · 正文：**无**——正文里没有这一条；这条本来就还没修，本条草稿就是它的全部记录

#### T36-①（低）· 已清 `503e427`
- **标题**：`tests/scratch_t33.rs` 被误提交进仓库
- **复现**：`git ls-files tests/`
- **修复前**：文件里两行注释自己写着「未被 git 跟踪、该 `rm` 掉」，结果 `git add -A` 连它一起带进了 `71a74d6`。不影响任何结果，但它是**一句写在仓库里的假话**
- **验收**：`git rm`，没有别的动作
- **状态**：已清

#### T36-②（中）· 已修 `503e427`
- **标题**：`status` 对「永远等不到」和「正常排队」用同一个词
- **复现**：造一条依赖 `deferred`/归档 todo 的活，看 `status` 的进展列
- **修复前**：三种命完全不同的等待印同一行灰的 `⏳ 等 t1`
- **验收**：死等和排队在面板上分得开
- **状态**：已修 · 回归测试 `cli_test::status_tells_a_dead_wait_apart_from_a_normal_queue` · 正文：[§14 第十一轮 · T36-②](CODE-AUDIT.md#t36-中status-对永远等不到和正常排队用同一个词--已修)

#### T37（中）· 已修 `bca786a`
- **标题**：「永远等不到」只在 `status` 一块屏上说了，另外三处紧凑清单还在印 `⏳t4`
- **复现**：同 T36-②，改看 `zloop context` / `prompt` 渲染 / `cmd_edit` 回显
- **修复前**：三处照旧印「排上了」·**并且 t36 自己的判据也漏了一种**：它取「第一条没 done 的依赖」再判死活，死依赖排在活依赖**后面**就整条漏掉（`doctor` 那边是把 `blocked_by` 整条扫完的）
- **验收**：四处一起说，且**判据只留一份**
- **状态**：已修 · 正文：[§14 第十一轮 · T37](CODE-AUDIT.md#t37中永远等不到只在-status-一块屏上说了--已修并补上-t36-漏判的那一半)

#### T38（中）· 已修 `0c27d16`
- **标题**：延后一条依赖 = 一条命令判死一片，而 `edit` 的回显只字不提
- **复现**：`zloop edit t4 --status deferred`（t2/t3 依赖 t4），紧接着 `zloop doctor`
- **修复前**：`edit` 只印 `t4 [P2] deferred 四` 就结束，`doctor` 退 1 报 t2/t3 永远轮不到——同一份 state 两块屏说的话对不上，而 `edit` 那一行是这一刻**唯一会被读到的**
- **验收**：回显当场点名被连累的那几条 + 敲什么解开；判据不另写（复用 `dead_deps`）；长清单只印前 8 个但条数说全；**退出码仍是 0**（`edit` 本身成功了，改成非 0 会打断脚本里的 `edit && …`）
- **状态**：已修 · 撤掉回显那段，新测试报「被连累的条数要说出来」 · 正文：**无**——正文里没有这一条，全部记录就是本条草稿（实现说明在修复 commit 里）

#### T39（中高）· 已修 `c441398`
- **标题**：`compact` 把还有人依赖的那条搬进归档，等它的那几条就此永远等不到
- **复现**：`plan` 三条 → `edit t2/t3 --blocked-by t1` → 做完 t1 → 做旧 → `zloop compact`
- **修复前**：t1 进归档，t2/t3 的 `blocked_by` 指着一个不存在的 id；**归档里的 todo 捡不回来**（这是它比 T38 狠的地方——T38 还能 `edit` 撤销）
- **验收**：还有人依赖的那条不许搬
- **状态**：已修 · 正文：[§15 第十二轮 · T39](CODE-AUDIT.md#t39中高compact-把还有人依赖的那条搬进归档等它的那几条就此永远等不到--已修)

#### T40-①（中高）· 已修 `331293a` `5304382`
- **标题**：例行 `compact` 吃掉人今天刚留下的、还没人读过的反馈
- **复现**：`zloop feedback t1 "…"`（t1 完成于 40 天前）→ `zloop compact --keep-days 30` → `zloop context | grep`
- **修复前**：1 条 → **0 条**。三点让它比 T39 更难发现：**不需要 `--force`**（最普通的例行整理，甚至能进 cron）；**静默**（回显只说「2 ticks」，`doctor` 前后都退 0）；**丢的正是协议里排第一的那个输入**（skill 的原话是「交接包里有反馈就先按它调整」）
- **验收**：搬 tick 的判据要多一条——这条 tick 自己也得够老，且 `pending_feedback` 指着的一个都不许搬（②③ 两道闸缺一不可）
- **状态**：已修 · 正文：[§16.2 T40-①](CODE-AUDIT.md#162-t40-中高例行-compact-吃掉人今天刚留下的还没人读过的反馈--已修见-17)、[§17 第十四轮](CODE-AUDIT.md#17-第十四轮compact-剩下的两处指针一起收口t40--已修)

#### T40-②（中）· 已修 `331293a` `5304382`
- **标题**：`compact --force` 把在飞的那条搬走，`ensure_idle` 给的两条出口从此都退 2
- **复现**：`zloop next` → `zloop edit t1 --status deferred` → `zloop compact --keep-days 0 --force`
- **修复前**：`done t1` 和 `edit t1 --status open` 双双 `unknown todo id`，`doctor` 退 1 且给的修法是「手工把 state.json 里的 in_progress 删掉」——**zloop 自己造出了一个只能手改文件才能收拾的状态**
- **验收**：`in_progress.todo` 不许进 `old_ids`，`--force` 也不许（`--force` 的语义是「我知道有人在跑，账我认」，不是「把在飞的那一轮删掉」）；出口只印真的能用的那条
- **状态**：已修 · 正文：[§16.3 T40-②](CODE-AUDIT.md#163-t40-中compact---force-把在飞的那条搬走ensure_idle-给的两条出口从此都退-2--已修见-17)、[§17 第十四轮](CODE-AUDIT.md#17-第十四轮compact-剩下的两处指针一起收口t40--已修)

#### T42（中高）· 已修 `2e0168f`
- **标题**：派活指着一条已了结的 todo 时，四处出口一起坏
- **复现**：`zloop next`（派出 t1）→ `zloop edit t1 --status deferred`。**两步都在最普通的用法里**，一道闸都不响，也不用改文件
- **修复前**：`done t1` 退 2 `already deferred`；`ensure_idle` 指的正是那条退 2 的命令；`status` 同一屏上「t1 ⏭ 已延后」+「正在做 t1」；`doctor` 一声不吭退 0。**最狠的是第一条落在无头轮次上**——runner 每轮塞给模型的收尾指令写死是「必须执行 `zloop done <id>`」，模型手里只有这一条命令，而它保证失败
- **验收**：让 `done` 收得了尾，且**不改状态**（另一条路是劝人撤销自己刚做的决定，否决）；闸只对在飞的那条开
- **状态**：已修 · 正文：[§18 第十五轮 · T42](CODE-AUDIT.md#18-第十五轮t42中高派活指着一条已了结的-todo-时四处出口一起坏--已修)

#### T43（中）· 已修 `b406ded`
- **标题**：`install_sudoers` 的暂存路径别人也占得住名字，装进 `/etc/sudoers.d/` 的可以不是我们写的那份
- **复现**：`sh scripts/repro-t43-sudoers-tmp-swap.sh`
- **修复前**：`env::temp_dir().join(format!("zloop-pmset.{}", process::id()))` —— 名字可猜、写的时候不 `O_EXCL` 不 `O_NOFOLLOW`，`visudo -c` 检查的和最后装进去的可以是两份
- **验收**：换掉的不是名字，是**父目录**（自己建一个 0700 的目录）
- **状态**：已修 ·**顺带纠正了一句写错的前提**：macOS 上「TMPDIR 没设 = /tmp」不成立，Apple 平台 `env::temp_dir()` 走 `confstr(_CS_DARWIN_USER_TEMP_DIR)`，拿到的是每用户 0700 的 `/var/folders/…/T/`（`env -i` 实测）。可利用性因此收窄，但修法不变 · 正文：[§20 第十七轮 · T43](CODE-AUDIT.md#t43中install_sudoers-的暂存路径别人也能占名装进-etcsudoersd-的可以不是我们写的那份--已修)

#### T44（中）· 已修 `71e5c8a`
- **标题**：整理一次账本，进度条 66% → 0%、「一次过 2/2」→「0/0」、`goals` 的 2/3 → 0/1
- **复现**：`sh scripts/repro-t44-compact-drops-progress-percent.sh`（修之前退 1）
- **修复前**：T29 修的是从 **ticks** 现算的那一族，从 **todo** 现算的还有两个没修（[§21.4](CODE-AUDIT.md#214-没修的那一半说清楚别当没看见) 明写过），这一轮做掉并发现第三个出口
- **验收**：归档汇总里再存一份「todo 那一侧的原料」；顺带修掉「整理干净的目标被说成刚开的」（`total == 0 && archived.todos == 0` 才算待规划）
- **状态**：已修 ·**这是同一个教训第三次出现**：为「空账本 = 刚开始」准备的早退分支，`compact` 都能伪造它的前提 · 正文：[§22 第十九轮 · T44](CODE-AUDIT.md#t44中整理一次账本进度条-66--0一次过-2200--已修)

## 4. 不修（查过，确认不是缺陷）

不进 issue，但写下来——因为它们都长得像缺陷，下一个人会重新怀疑一遍。

| 事 | 为什么不修 |
|---|---|
| `--evidence @<大文件>` 不设上限 | 32 MB 证据实测：日志涨到 32 MB、峰值内存 132 MB，但**账本和交接包都没被污染**（`state.json` 1079 B、`tick.note` 1 字符、`context` 540 B）。该有的边界都在，只是落盘那份没截 |
| `plan --file /dev/zero` 会挂住 | `read_to_string` 无界读，但要拿**字符设备**喂它才触发，场景太构造 |
| `zloop log --show ../../../../etc/hosts` | 本地 CLI，用户对这些文件本来就有读权限，**不构成越权**；`--show` 的帮助文字写的就是 "path or bare file name" |
| 孤儿日志文件（tick 进归档、`.md` 留在原地） | `doctor` 的 `missing_log` 查的是反方向；文件还读得到，没有任何路径会崩 |
| NOTES.md 会不会是 compact 的第三个受害者 | `notes.rs` 存的是 `- <RFC3339> 正文`，**没有 todo id 这个概念**，compact 从不碰这个文件。受害者不存在 |
| `ensure_idle` 的 TOCTOU | 赢了竞态也没用：`next` 派出去的那条状态是 `open`，而 `old_ids` 只收 `is_terminal` 的。直接把竞态结果摆出来试 → `nothing to compact`、todo 还在、`doctor` 没发现问题 |
| `awake.rs` 剩下 3 处裸子进程 | 收口会把功能弄坏（收尾路径上的探针不能认叫停，见 T21）。理由写进了**代码注释**，不只写在文档里 |
| `edit`/`feedback` 打断 **noop** streak | 有意：`noop` 不是停机闸，且 A-16 之后 runner 不读它 |
| `reflect` / `replan` tick 对 streak 透明 | 有意（`tick.rs` 写明：插一轮反思不代表失败被解决了） |
| `held_by_other` 挡不住 runner | 有意且**必须**：runner 自家的 `claude -p` 子进程要靠这条放行才进得来 |
| `goal new` / `switch` / `rm` 换掉整个 `state.json` | `goals::ensure_idle` 已经挡住了（runner 在跑、或有轮次没写回都拒绝，除非 `--force`） |

## 5. 试过但没复现（不进 issue）

写下来是为了让下一个人别重复试。**这些不算发现**——混进 issue 会污染真发现。

- 并发写账本丢更新（20 并发 `plan --add` → 20/20，id 不重复）；
- SIGKILL 把 `state.json` 写坏（386 次真 SIGKILL，每轮验 JSON，一次没坏）；
- 残留锁文件把后续命令锁死（flock 由内核在进程死亡时释放）；
- 日志文件和 tick 对不上（`log::write` 在事务闭包**里面**；最坏是孤儿日志，而 `log::entries` 按 tick 过滤）；
- 路径穿越写到 `.zloop/` 外面（`goal new --id ../../pwn` 被 `sanitize_id` 压成 `pwn`）；
- 环境变量把输出搞崩（`COLUMNS` 喂 0/1/2/-5/abc/20 位数，加 `NO_COLOR`/`CLICOLOR_FORCE`/`ZLOOP_AWAKE_POLL_SECS=abc`，9 种全 exit 0 且行数不变）；
- stdin 喂垃圾（`hook-stop` 喂空/乱文本/`[]`/2000 层嵌套 → 全 exit 0 静默忽略）；
- 超长 / 控制字符的 CLI 参数（1 MB 目标文字、`\x1b[31m`、`\x07` → 正常收下、正常存）；
- 非 UTF-8 从命令行钻进来（clap 直接挡）——**唯一漏的是 NOTES.md，那条是 A-4**；
- `cargo test` 漏进程（两遍全绿、临时进程都收掉了；机器上那个活了 20 小时的 runner 是 A-5 不是漏进程）。

## 6. 这张表自己的边界

三件说清楚，别当没说：

1. **「没有 bug」仍然证不出来。** 这 42 条是**这几轮按风险面扫到的**，不是全集。
   没扫到的面（真实并发的宿主进程、非 macOS 平台、`.zloop` 落在网络文件系统上）
   一条都没验过。
2. **41 条"已修"里，每条的红/绿证据都在各自正文里**，形式不统一：多数是"撤掉修复就变红"
   的回归测试，少数（A-13 / T21 的取舍类）是逐条比对的实测记录。
   引用这张表时请连正文一起看，别只看"已修"两个字。
3. **本次不开 GitHub issue、不 `git push`**（项目约定）。这一节就是那份等人看的清单；
   要落成真 issue 时走 `scripts/gh-issues.py`，绑定约定是 todo 文本结尾带 `(#N)`。