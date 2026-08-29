# 自进化回路：从 Warp 抄什么、怎么抄

> 目标：`参考 warp 的 self-improvement loops … 帮我优化一点现有的 zloop`。
> 博客对照（缺口 W1–W6）已经在 `GOALS-REVIEW.md` 的「对照 Warp 的自我改进 agent」一节，W1（`zloop feedback`）已实现。
> 本文只记**源码那一路**：`github.com/warpdotdev/warp`，master @ `49db2315`（2026-08-28），以及它对 W2–W6 设计的具体影响。

## 0. 先划红线：许可

| | zloop | warpdotdev/warp |
|---|---|---|
| 许可 | MIT | **AGPL-3.0** |

**所以不能抄代码，只能看设计。** AGPL 的传染性会让任何被复制进来的实现污染 zloop 的 MIT 许可。
本文里的引用都是为了说明机制的短摘录，落到 zloop 的实现必须自己写。这一条写在最前面，
是因为「有源码可看」很容易变成「顺手粘一段」。

## 1. 抄到了什么（四条，都有实物）

### 1.1 「定时跑的观察者」= cron + prompt，就这么简单

博客说 improver skill 是 scheduled agent；源码里这个东西的数据模型只有六个字段
（`crates/cloud_object_models/src/scheduled_ambient_agent.rs`）：

```rust
pub struct ScheduledAmbientAgent {
    pub name: String,
    pub cron_schedule: String,        // cron 表达式
    pub enabled: bool,                // 能关
    pub prompt: String,               // 跑什么，就是一段提示词
    pub last_spawn_error: Option<String>,  // 上次为什么没起来
    pub agent_config: AgentConfigSnapshot,
}
```

`AgentConfigSnapshot` 里值得注意的两个字段：`skill_spec: Option<String>`（这一跑用哪个 skill）、
`harness: HarnessConfig`（claude / codex —— 和 zloop 的 host 是同一个概念）。

CLI 侧同一件事（`resources/bundled/skills/oz-platform/SKILL.md`）：

```sh
$ warpctrl schedule create --cron "0 8 * * *" \
    --name "GitHub issue summary" \
    --prompt "Collect all feedback from new GitHub issues and provide a summary report" \
    --environment UA17BXYZ
$ warpctrl schedule list / schedule get <id>
$ warpctrl run list / run get <run-id>
```

**对 W6 的影响**：确认了"定时反思"不需要新子系统——它就是「按点跑一段不同的 prompt」。
zloop 已经有 runner + `intervals_min`，`--reflect-every N` 就是"每 N 轮把 prompt 换一份跑一轮"。
两个字段值得照抄：**`enabled`（能单独关掉，不必停整个 runner）** 和
**`last_spawn_error`（记下上一次为什么没跑起来，而不是静默不跑）**——后者正是 zloop 现在缺的那类可观测性
（对比：空目标 `zloop start` 会启动后立刻 `stop (all_done)`，没人告诉你原因是"还没规划"）。

### 1.2 「改进 skill」在 Warp 那边是一条 eval 回路，不是一次聪明的改写

`resources/bundled/skills/create-skill/SKILL.md`（这份 skill 自带 LICENSE.txt）把过程写得很直白：

> - Decide what you want the skill to do … Write a draft of the skill
> - Create a few test prompts and run the agent with access to the skill on them
> - Help the user evaluate the results both **qualitatively and quantitatively**
> - **Rewrite the skill based on feedback from the user's evaluation** …
> - Repeat until you're satisfied
> - Expand the test set and try again at larger scale

而且这条回路是**真有工具的**，不是一段口号。`create-skill` 自带：

```
scripts/run_eval.py            跑一批测试 prompt
scripts/run_loop.py            把"跑 → 评 → 改"串成循环
scripts/aggregate_benchmark.py 汇总多次运行（博客里提到的方差分析）
scripts/generate_report.py     出报告
scripts/improve_description.py 单独优化 skill 的 description（触发准确率）
scripts/quick_validate.py      快速校验
eval-viewer/generate_review.py + viewer.html   给人看结果
```

同时明确保留了人的旁路——用户说"不用跑一堆评估，随便聊聊"就那样做。

**对 W2 的影响**：改进的输入是**定量指标 + 人的评价**两条腿。zloop 没有 eval 集，短期也不该造一个；
所以 reflect 的输出只能是**建议**、由人点头（原设计不变，这次是拿到了证据）。
但"定量"那条腿不能空着——`zloop stats`（W5）就是它的位置：**先有数字，reflect 才有得读**。
这也决定了 t4（stats）必须排在 t5（reflect）前面。

### 1.3 skill 是**被版本锁住的依赖**，安装器绝不静默覆盖

`AGENTS.md` 里关于 `skills-lock.json` 的那一段（skill 从另一个仓库 `warpdotdev/common-skills` 装进来，
`npx skills` 管版本）：

> 装之前必须给出明确目标（`--project` / `--global` / 环境变量 / 交互回答），**非交互流程在目标不明确时直接失败**；
> 如果 project 和 global 两处都有 common skills 就**报错**；
> 阻止"一个 checkout 的 global 安装被另一个 pin 到不同 lock 的 checkout 静默覆盖"；
> 装完（或跳过）之后**按 lock 校验已安装的 skill**。

**对 W3 的影响**：这比我原来想的"留一个用户区块"更有力。Warp 的安装器把
「目标不明确 → 失败」「两处都有 → 报错」「可能静默覆盖 → 阻止」「装完 → 校验」写成了硬规则。
而 zloop 的 `install` 恰好相反：带 `zloop-managed` 标记的文件**无条件重写**，输出只有一行 `wrote`
（`hosts.rs:67`，实测见 `GOALS-REVIEW.md` 的 W3）。所以 t2 的验收标准要加一条：
**安装器发现自己会覆盖用户内容时必须停下并说清楚**，而不只是"保住用户区块"。

### 1.4 base skill 的写法：硬上限 + 指向唯一真源

`.agents/skills/` 下有 21 个 SKILL.md，正是博客里 inner/base skill 那一层的实物
（`triage-issue-local`、`review-pr-local`、`dedupe-issue-local` 都在）。两个可以直接学的手法：

- **硬上限写成数字**：triage skill 有一节就叫 "Follow-up question limit"——
  "Ask **at most 2 follow-up questions** per triage response"，还解释了什么叫 high-value 的问题。
- **不在 skill 里复制事实，只给指针**：label taxonomy 明确说"managed in `.github/issue-triage/config.json`，
  优先用那里的标签，**不要自己发明**"。

**对 zloop SKILL.md 的影响**：目前模板里"把目标拆成 2–5 条可验证的 todo"已经是这种写法，
但"证据放 `--evidence`""每轮只做一条"这类规则还是散文。可以在 t2 顺手把它们改成带数字的硬约束。

### 1.5 「这一跑是谁发起的」是一等分类

`app/src/ai/ambient_agents/task.rs` 里的 `AgentSource` 枚举，把 run 的来源列成了明确的几类
（左边是内部名，右边是对外暴露的名字）：

```rust
AgentSource::RunScorer      => "RUN_SCORER"      / "Scorer"
AgentSource::Autofix        => "SELF_IMPROVEMENT" / "Self-improvement"
AgentSource::BenchmarkTrial => "BENCHMARK_TRIAL" / "Benchmark"
AgentSource::GitHubWebhook / GitLabWebhook / AgentWebhook("API") / …
```

两件事从这里读出来：

1. **自改进是一类 run，不是一个副作用**——它和"人点的""webhook 触发的"平级，各自能统计、能筛。
2. 更有意思的是 **`RunScorer`**：他们有一个专门给 run 打分的东西，而自改进（Autofix）与基准（BenchmarkTrial）
   都是围着分数转的。也就是说完整回路是 **跑 → 打分 → 自改进 → 基准复测**，
   "打分"是自改进的前一环。

**对 W5 / W6 的影响**：
- zloop 的 `tick.via` 现在只有 `"next"` / `"runner"` 两种值。reflect 轮次应该是**第三种 via**（比如 `"reflect"`），
  这样它天然不算进 todo 轮次、也能在 `stats` 里单独统计——不需要额外的表。
- `RunScorer` 提醒了 W5 缺的那一半：zloop 现在只有 `tick.documented`（有没有留实现思路，一个布尔），
  这是最原始的打分器。`zloop stats` 除了"多少轮、花多少钱"，还应该给出**每条 todo 的返工次数**
  （progress 轮数 ÷ 最终 done）这类质量信号——那才是 reflect 读得懂的输入。

## 2. 明确**没有**的东西（别再猜）

- **improver skill 本体不在开源仓库里。** 全仓搜 `improver` 只命中 `create-skill/SKILL.md` 一处；
  `.agents/skills/` 的 21 个 skill 里没有任何一个是"读反馈、改别的 skill"的观察者。
  博客描述的那个 outer skill 跑在 Oz 上，仓库里只有它的**调度机制**（1.1）和**通用改进方法论**（1.2）。
- **没有可抄的"反馈聚合"实现。** `.agents` 里搜 `feedback` 只有 3 处命中，都是文档链接或无关上下文，
  不是"把人类回应喂回 skill"的代码。
- **也就没有必要再翻这个仓库第二遍。** 结论已经够定 W2–W6 的设计了；
  再深入就是读 Warp 的 GUI/TUI 实现，和 zloop 无关。

## 3. 这一轮对计划的三个具体修正

| 待办 | 原来的想法 | 看完源码之后 |
|---|---|---|
| t2（W3 install） | 给 SKILL.md 留一个用户区块 | **加一条：会覆盖用户内容时必须停下报错**（Warp 安装器的四条硬规则）；顺手把模板里的软规则改成带数字的硬约束 |
| t4（W5 stats） | 一个好用的只读命令 | **它是 reflect 的前置**：Warp 的回路是"跑 → 打分 → 自改进"，`RunScorer` 就在自改进前一环。stats 除了轮数/花费，要给出**返工率**这类质量信号（现有优先级已经是对的） |
| t5（W2/W6 reflect） | 一条命令 + runner 间隔 | 形状确认：**间隔 + 一段不同的 prompt + 能单独关 + 记下上次为什么没跑起来**（`enabled` / `last_spawn_error` 两个字段照抄）；reflect 轮次用**第三种 `tick.via`**（`"reflect"`），天然不占 todo 轮次也便于统计；输出仍然只是建议，人点头才落地 |

依据（都在 master @ `49db2315`）：
`crates/cloud_object_models/src/scheduled_ambient_agent.rs`、
`app/src/ai/ambient_agents/{scheduled,task,spawn}.rs`、
`resources/bundled/skills/oz-platform/SKILL.md`、
`resources/bundled/skills/create-skill/SKILL.md`（+ `scripts/improve_description.py`）、
`AGENTS.md`（`skills-lock.json` 那一段）、
`.agents/skills/triage-issue-local/SKILL.md`。

---

# W3 落地：install 不再吃掉你对 SKILL.md 的改动（t2）

## 改成了什么

SKILL.md 切成两半，中间一条线：

```markdown
<!-- zloop-managed:v1 fp=951d4b55f04640e8 -->   托管区（install 负责更新，指纹钉住内容）
…
<!-- zloop:user -->                              用户区（install 永不改动，原样搬走）
```

| 情况 | 旧行为 | 新行为 |
|---|---|---|
| 用户在用户区写东西 | 没有用户区这个概念，整份重写 → 内容消失 | 原样保留，输出报告保留了多少字节 |
| 用户手改了托管区 | 静默覆盖，输出只有一行 `wrote` | **停下报错**（退出码 2），指出两条出路：搬到用户区，或 `--force` |
| 什么都没改的重装 | 比较全文，一样就 `kept` | 同左（但要小心切分，见下面的坑） |
| 旧版装的文件（无指纹） | —— | 覆盖一次并说明，从此保护生效 |
| `agents/openai.yaml` | 内容不同就直接盖 | 同一套指纹保护（YAML 注释形式），没有用户区 |

## 为什么是指纹，而不是"和模板比一比"

"托管区被改过"不能靠"和当前模板不一致"来判断——**模板自己会升级**，那样每次升级都会被当成用户改动。
所以标记行里记下写入时的内容指纹，读回来重算一遍：一致 = 我写的，不一致 = 有人动过。

指纹用自己实现的 FNV-1a 64（`hosts.rs fingerprint`），不引入哈希库：它只回答"这段文字还是我写下的那段吗"，
不做安全用途。**特意不用 `DefaultHasher`**——它的取值没有跨 Rust 版本/平台不变的保证，
一旦 rustc 换了种子，所有人的 install 都会突然误判成"用户改过"。

## 踩到的两个坑（都出在"两边算的不是同一段文字"）

1. **归一化必须两边一致。** 第一版写入时对 `FRONTMATTER + body` 算指纹，读回来时对"文件里切到 `USER_MARK` 为止的那段"算——
   后者末尾多一个换行（用户区起始的那个），于是**每次重装都报"被改过"**。
   修法：写入和读取都走同一个 `fp_of()`，内部 `canonical()` 剔掉标记行并 `trim_end()`。
2. **切分要连前面那个换行一起切给用户区。** 否则"什么都没改"的重装重建出来的文件差一个字节，
   每次都打印 `wrote`，看着像它动了文件。

这两条是同一个教训的两面：**凡是"写出去的值"和"读回来重算的值"要比较，就必须保证两边经过完全相同的处理**——
最好像这里一样，物理上只有一个函数。

## 验证

`cargo test` 81 passed / 0 failed，新增 `install_keeps_your_edits_and_refuses_to_clobber_the_managed_part`，
四种情况逐条断言（用户区保留且文件零改动 / 改托管区被拒且文件未被动 / `--force` 覆盖托管区但用户区仍在 / 旧文件迁移）。
实机六种场景也跑过：干净安装、无改动重装（`kept`）、用户区写东西后重装、改托管区被拒（退出码 2）、
`--force`、旧版裸标记文件迁移，以及 codex 侧的 SKILL.md + openai.yaml。

---

# W4 落地：让失败变成"学到"（t3）

## 问题

`fail_streak >= 3` 会让循环停下来等人——这是对的。但**停下来不等于学到**：
旧实现里 `--outcome fail` 只要一句自由文本 `--note` 就能写回，失败的原因没有任何结构化落点。
下一轮（甚至下一个会话）拿到的交接包里只有一行"最近 3 次执行"的摘要，同一个坑完全可能再踩一遍。

## 改成了什么

1. **`--outcome fail` 必须带 `--pitfall`**（`policy.require_pitfall`，默认 `true`，`--no-doc` 可绕过一次）。
   和 `require_doc` 同一个机制、同一个位置：**在任何状态写入之前**检查，所以被拒的调用什么都没改，补参数重跑即可。
2. **坑同时进账本**：`Tick.pitfalls: Vec<String>`。日志文件里本来就有渲染版，但账本里再存一份是有意的——
   `context` 要能直接读出"这个目标失败过的地方"，而不必回头解析 Markdown；更重要的是
   **账本跟着目标走，日志目录是项目级的**（多目标下这一点已经吃过亏，见 `GOALS-REVIEW.md` F5）。
3. **`context` 多一节「本目标失败过的地方（别重复踩）」**：最近 3 次 fail/block，每条带最多 2 个坑，
   排在「下一条」**前面**，并计入不可裁剪的保护区（和「用户反馈」一样）。

```
## 本目标失败过的地方（别重复踩）
- 2026-08-29T06:48:47+08:00 t1 失败：cargo build 在 M1 上链接失败
  ↳ 坑：sqlite3 要用 brew 那份 libsqlite3，系统自带的缺符号；下次先 otool -L 看链的是哪个
- 2026-08-29T06:49:00+08:00 t2 卡住：压测跑 CI 还是本地？
```

## 两个刻意的选择

- **不印轮次号。** `tick.round` 只在 done/progress 时递增（`record` 的 `bump`），所以失败那一条的 round
  是"上一轮的编号"，印出来是"第 0 轮失败"这种读不通的话。改印时间戳，顺便回答了"多久以前"。
- **runner 自己记的 fail 不受这条约束。** preflight 不过、宿主超时、被限流——这些是机械故障，
  runner 直接 `tick::record("fail", …)`，没有坑可写。它们照样出现在失败清单里，只是没有 `↳ 坑` 那一行。
  强行要求 runner 编一个坑出来只会污染这份清单。

## 验证

`cargo test` 82 passed / 0 failed，新增 `a_failed_round_must_leave_a_pitfall_and_it_shows_up_in_context`：
不带坑退出 2 且**不留 tick**、报错里有可直接抄的重试命令、带坑后 `tick.pitfalls` 落地、
context 里出现且排在「下一条」之前、block 也进清单、`--no-doc` 与 `policy.require_pitfall=false` 两条旁路都放行。

顺带修了三处旧测试：它们写 `--outcome fail` 时不带坑（`end_to_end`、文档闸那条、`feedback_breaks_the_fail_streak`）——
其中最后一条如果不补，三次 fail 根本没被记下来，`fail_streak` 测的东西就不存在了。
**改了写回的前置条件，要回头看所有构造该状态的测试**，否则它们会静默地测了个空。

---

# W5 落地：`zloop stats`（t4）

## 它补的是哪一环

Warp 的回路是 **跑 → 打分 → 自改进 → 基准复测**，`AgentSource::RunScorer` 就在自改进的前一环（§1.5）。
zloop 此前的"打分器"只有 `tick.documented` 一个布尔值——回答了"交没交作业"，没回答"做得顺不顺"。
`stats` 把这一环补上：**它的第一读者是 reflect（W2/W6），人只是顺带看**。

## 分工

| 命令 | 回答什么 |
|---|---|
| `status` | 还剩什么、我现在该敲什么 |
| `stats` | 跑得顺不顺、钱花在哪、哪一步最费劲 |
| `log` / `doc` | 具体那一轮发生了什么 |

## 指标怎么定的

全部从 `state.ticks` 现推，**不新增任何存储**——账本本来就记着每一轮。

| 指标 | 定义 | 为什么是这个定义 |
|---|---|---|
| 轮次 | `done`+`progress`+`fail` 的 tick 数 | 和 `tick::COUNTED`、配额窗口同一口径；`noop`/`edit`/`feedback` 不是"干活" |
| 返工 | `progress`+`fail` 的轮数，以及它占轮次的比例 | 一条 todo 反复没做完，就是这个循环最主要的浪费 |
| 一次过 | 一轮做完且没返工过的 todo ÷ 已完成的 todo | 比"完成率"有信息量：完成率最后总会到 100% |
| 无文档 | `documented == Some(false)` 的轮次 | 和 `zloop log` 的 ⚠ 同源 |
| 最费劲 | 返工最多的那条，其次失败、被挡 | reflect 的第一个提问对象 |
| 花费 | 只在宿主报过 `cost_usd` 时才显示 | 交互式轮次没有这个数，硬显示 `$0.00` 是噪声 |

## 两个实现上的决定

- **`style::table()` 抽成了通用件。** `status` 的清单表是专用的（有子行、有溢出命令、有配色），
  但 `stats` 只要一张普通表，再抄一遍框线算宽的代码不值当。新的 `style::table(head, rows, align, flex, budget, c)`
  按 `width()` 算列宽、指定哪一列可压缩、支持左右对齐。`status` 那张暂时没动——它的需求确实更复杂，
  等 reflect 也要出表时再看要不要合。
- **数字必须能被独立验算。** 验收标准里"数字与 state.json 对得上"不是走过场：
  回归测试 `stats_counts_match_the_ledger` 是**从 `state.json` 现推一遍**再和 `--json` 逐字段比，
  而不是把实现里的表达式抄进断言——后者只会证明代码等于它自己。

## 验证

`cargo test` 83 passed / 0 failed。实机构造了一个"有返工有失败"的目标（1 条一遍过、1 条两次 progress 后完成且无文档、
1 条 fail、1 条 block、外加 1 条用户反馈），8 个汇总字段 + 每条 todo 的轮次全部和 `state.json` 现推的数字一致；
表格各行等宽；没跑过的目标显示"还没有跑过任何一轮"而不是一堆 0。

---

# W2 + W6 落地：reflect（t5）

## 形状：一段不同的 prompt，不是一个新子系统

Warp 的 improver 是**按计划跑的观察者**，数据模型只有六个字段（§1.1）。所以 zloop 这边也不该长出
调度器、评分服务、规则引擎——回看就是"隔一阵子换一段 prompt 跑一轮"：

| | 手动 | 无头 |
|---|---|---|
| 怎么触发 | `zloop reflect` | `zloop start --reflect-every N` |
| 谁判断 | 模型（读材料包） | 模型（同一份材料包 + 一句"没人点头") |
| 怎么落地 | `zloop reflect --apply`（人点头后） | **不落地**，把建议记进账本等人回来看 |

## zloop 负责什么、不负责什么

**负责**：把材料摆齐（全部经验带日期、失败与坑、用户说过的话、`stats` 的几个数字），
再做几项**代码能判断**的体检：

- 哪两条经验像是同一件事 —— 字符集合的**重合系数** ≥ 0.8（交集 ÷ 较短那条）。
  用重合系数而不是 Jaccard：真实场景多半是"同一条经验后来写得更细了"，长度差得多，
  Jaccard 会把这种明显重复判成不像。宁可多提示——**这里只给候选**，误报的代价只是模型多看一眼。
- 有几条已经被 `zloop context` 的窗口（最新 5 条）挡在外面 —— 写到第 20 条时，前 15 条对模型等于不存在。
- **约定攒得太多了没有** —— 超过 10 条就提一句（`zloop reflect --max-rules N` 可调）。
  经验有窗口兜底，约定**没有**：它不轮换，写多少条就每轮全量占多少交接包篇幅，
  而被挤掉的正是从尾部往前裁的那几节（经验、待办、怎么继续）。所以提示里除了条数，
  还把实际占的字数和**占默认预算的百分比**算出来——「11 条」听着不多，「占 5%」才是代价。

**不负责**：判断。合并哪几条、删哪一条是模型的事；**落地要人点头**。
Warp 那边人审的形态是 PR review，zloop 没有 PR，所以就是 `--apply` 这一步。

## 四个刻意的选择

1. **回看是第 8 个 outcome，对三条 streak 透明。** 如果 `reflect` 像普通轮次那样打断 fail_streak，
   `fail / fail / reflect / fail / fail …` 会让循环永远停不下来——插一轮反思不等于失败被解决了。
   它也不进 `COUNTED`：不吃配额窗口、不推进轮次编号。
2. **无头回看绝不自己改 NOTES.md。** 没人点头的时候，最多只能建议。它把输出记成一条 `reflect` tick
   （完整正文写进 `.zloop/log/…-reflect.md`），`zloop log` 里看得到。
3. **改 NOTES 之前先备份**（`NOTES.md.bak-<时间戳>`）——照 Warp 的 `mutate_global_registry` 的做法。
   这是 zloop 里唯一一个会删掉用户内容的操作。
4. **空 stdin 不当成"清空"。** `--apply` 收到空输入直接退出 2；要清空得显式给内容。

## 顺手修的一个旧问题

`notes::recent()` 的文档说返回值"without the timestamp prefix"，但它其实没剥——
所以 `zloop context` 里每条经验都带着完整的 RFC3339。现在按文档剥掉了（`split_stamp`），
交接包短了一截；`reflect` 的材料包只显示 `[08-29]` 这样的日期。
这也是必须的：不剥的话，模型抄回来的清单会带上旧时间戳，`--apply` 之后变成双时间戳；
而且比较两条经验像不像时，日期字符会把相似度整体抬高。

## 验证

`cargo test` 86 passed / 0 failed，新增三条：

- `reflect_gathers_the_material_and_only_lands_when_you_say_so`：材料包四节齐全、坑与用户反馈都在、
  体检认出重复、经验不带 RFC3339、**只读**（跑两次输出一致）、`--apply` 容忍编号和短横线、
  备份生成、下一轮 `context` 带上新的、空输入退出 2 且不动文件；
- `reflect_every_inserts_a_round_that_does_not_consume_a_todo`（假宿主）：3 轮里正好插 1 次回看、
  它不挂在任何 todo 上、宿主输出进了账本和日志、三条 todo 照样全做完、轮次编号仍是 3、NOTES.md 没被创建；
- `a_reflect_round_does_not_reset_the_fail_streak`：3 次 fail 之后插一轮回看，`fail_streak` 仍是 3。

第三项体检（约定太多，#10）后补，`cargo test` 96 passed / 0 failed，再加一条：

- `reflect_flags_too_many_rules_and_the_threshold_is_tunable`：正好 10 条不出声、11 条自动提、
  `--max-rules` 调低调高都认、篇幅算成真数字（233 字 / 5%）、「你要做的」那句劝跟着阈值走、
  体检只读不动 NOTES、和另外两项体检互不干扰。六个变异（去掉体检 / `>` 改 `>=` / 改默认阈值 /
  忽略 flag / 少算「- 」和换行 / 把劝的话写死）逐个打进去，测试全都转红。

---

# 回路的最后一段：约定这一层（t6）

## 提问：Warp 那套真的融合进来了吗

W1–W6 做完之后自查了一遍，答案是**差一段**。Warp 那套的核心不是"记下经验"，而是
**improver 提出一个对 base skill 的小改动 → 人合并 → 下一轮 agent 继承**（§1 的第 4 条）。
而当时 zloop 的状态是：

- `reflect --apply` 只写 `.zloop/NOTES.md`（`reflect.rs` 里一次都没提过 skill）；
- t2 造出来的 SKILL.md 用户区**没有任何命令会写它**——全仓只有两处提到 `USER_MARK`，都是打印提示；
- 而 NOTES 每轮只带最新 5 条，写到第 20 条时前 15 条对模型等于不存在。

所以改进落不到"下一轮一定会读的东西"上。回路的最后一段是断的。

## 但也不能照抄"写进 skill"

`~/.claude/skills/zloop/SKILL.md` 是**全局**的——一个 host 一份，所有项目共用。
把"这个项目 done 之前要跑 cargo test"写进去，会跟着模型进到别的项目里。
Warp 的 skill 在**仓库**里（`.agents/skills/`），天然是项目级的；zloop 的不是。

所以真正缺的不是"写进 skill"，而是一个**项目级、每轮必读、不轮换**的位置。

## 做法：NOTES.md 分两层，不加新文件

```markdown
## 约定（每轮都带）          ← 全量注入交接包，多少条都带
- done 之前一定要跑 cargo test

## 经验（最近 5 条会带）      ← 会轮换
- 2026-08-29T07:00:00+08:00 bench.sh 要在 release 模式下跑
```

- **不新增文件、不新增概念**：还是那一个 `.zloop/NOTES.md`，还是 `remember` 写、`reflect` 整理。
- **老格式照旧能读**：没有小标题的一串 `- ` 全部当经验（`notes::parse`）。
- **`reflect` 的材料包**分两栏展示，并在窗口外的经验后面标「（窗口外，模型看不到）」——
  让模型看见"这条其实没人读得到"，这是升格的主要理由。
- **`--apply` 认同样的两个小标题**：模型读到什么形状，就写回什么形状。
- **`context` 把约定放在紧跟目标的位置**，全量注入。

## 顺手改掉的一个易错设计

交接包的裁剪保护原来是数出来的（`protected = 3 + 有没有反馈 + 有没有失败`），
每加一节就得回头改一次这个数字——W1 加反馈那次改过一回，W4 加失败又改过一回。
现在改成**按位置**：`sections.push(下一条)` 之后记 `protected = sections.len()`，
"到「下一条」为止的都不裁"。以后再插一节，这里不用动。

## 还差什么（说清楚，不含糊）

- **`remember` 不能直接写约定**：目前唯一的升格路径是走一次 `reflect`。
  想"顺手钉一条规矩"的话还得绕一圈——一个 `remember --rule` 就能解决，没做，因为 t6 的验收没要求。
- **跨项目不继承**：约定是项目级的（有意），全局 SKILL.md 的用户区仍然只能手改。
  Warp 那边靠 `skills-lock.json` 从 `common-skills` 仓库分发，那是团队规模的做法，zloop 不抄（§2）。
- **eval 集**依旧不做，理由见 §1.2 和 t1 的判断：单人单项目没有统计量。

## 验证

`cargo test` 87 passed / 0 failed，新增 `a_lesson_can_be_promoted_to_a_rule_that_ships_every_round`：
整理前交接包如实说"另有 3 条更早的没带上"、材料包标出窗口外的经验并教模型怎么写回、
`--apply` 报「约定 0 → 2 条 · 经验 8 → 2 条」、**再写 6 条经验把窗口挤满之后约定照样每轮都在**、
篇幅压到 700 字符时约定和「下一条」都还在、老格式仍按经验读。

---

# 把那个「差」摆出来（t7）

## 为什么必须配对

Warp 的 improver 干的事是：*"pulls the accumulated human feedback, compares **what the agent suggested**
against **how humans responded**, and proposes a small, focused edit to the base skill."*

关键词是 **compares**。在此之前 zloop 的材料包把 agent 自述（失败与坑）和用户反馈分成**两栏各列一遍**——
两栏都在，但"差"不在：模型看不到"我当时说了 X，人回了 Y"。

## 做法

`reflect::pair_feedback()`：每条 `feedback` tick 往前找**同一条 todo 上、它之前最近的那个已写回轮次**
（`COUNTED` 里的 done/progress/fail），配成一对：

```
### t1 · 2026-08-29T07:35:31+08:00
- 我当时说：用正则实现了（实现思路：正则最快，输入看着很规整）
- 你回的：正则不行，输入会有嵌套括号，换成手写状态机
```

- 「我当时说」= 那一轮的 `tick.note` +（有的话）日志里 `## 实现思路` 的开头 120 字。
  approach 不在账本上、只在日志文件里，所以新增了 `log::read_section(root, rel, title)` 回头取一次——
  **只对有反馈的那几轮调用**，量很小。
- **没人回过话的轮次不出现**。这一节的价值全在"有差"，把没差的也列出来只会稀释它。
- 配不上的（这条 todo 还没写回过任何一轮）照实说"这条反馈之前没有已写回的轮次"，不硬凑。
- 最近的排在前面，最多 8 对。

这一节替换了原来的「用户说过的话（全部）」——同一份信息，配对之后信息量高得多。

## 验证

`cargo test` 88 passed / 0 failed，新增 `reflect_pairs_what_i_said_with_what_you_replied`：
配对块同时含一句话结果和实现思路摘要、紧跟人的原话、最近的在前、
没人回过话的 t3 完全不出现、配不上的 t4 照实说明。

---

# 顺手钉一条：`remember --rule`（t8）

约定这一层做出来之后，唯一的入口是走一整轮 `reflect`——想"顺手钉一条规矩"得绕一圈。
`zloop remember --rule "<一句话>"` 直接写进约定区，立刻每轮生效；重复钉同一条会被识别，不会重。

**只给人用，不写进每轮协议。** 模型该走的仍然是 `reflect`（建议 → 人点头 → `--apply`）——
约定每轮都注入、永不轮换，让模型能单方面往里加东西，人就退出环外了。
`--rule` 是给人的快捷方式，不是给模型的。

**加约定不备份。** `reflect --apply` 会删东西，所以必须留原件；`--rule` 是纯增量，
每次都备份只会把 `.zloop/` 塞满。区别写在 `notes::add_rule` 的文档注释里。

## 顺带修掉一处无声的信息损失

`Lesson` 原来存的是 `MM-DD`（显示要用的那一半），而重写文件时要把日期写回去——
于是每次 `remember --rule` 触发重写，**所有经验的时刻都被抹成 `00:00`**。
改成存**完整时间戳**、显示时用 `notes::day_of()` 现算。
教训还是那条：**只存"要显示的那一半"，会在下一次重写时把另一半丢掉。**

`cargo test` 89 passed / 0 failed，新增 `remember_rule_pins_a_convention_without_a_reflect_cycle`：
钉两条 / 重复钉被识别 / 经验没被动 / 不产生备份 / **原始时间戳原样保留** / 立刻出现在 context / 空话退出 2。

---

# 一个用起来才撞到的 papercut（t9）

写 t8 那条 `zloop done` 时，`--decision` 的正文以 `--rule` 开头，被 clap 当成未知 flag，**整条命令被拒**。
记录"哪个 flag 不该用"这类坑时，正文以 `--` 开头是常事——而这恰恰是 `--pitfall` 最该记的东西。

修法：给所有装**人写的散文**的参数加 `allow_hyphen_values = true`——
`done` 的 note / approach / decision / pitfall / evidence / block / next，
以及 `init`、`plan --add`、`edit --text|--acceptance`、`remember`、`feedback`、`goal new`、`notify` 的文本。
路径和枚举类参数**不加**（它们不该以 `-` 开头，加了只会掩盖打错）。

**代价是有界的**，实测确认过两点：

- 打错的 flag 值照旧报错：`--outcome faill` → `invalid value 'faill'`（`value_parser` 仍然管用）；
- 漏写值也不会被悄悄吞掉：`--note --approach "思路"` → `unexpected argument '思路' found`，
  而不是把 `--approach` 当成 note 存进去。

`cargo test` 90 passed / 0 failed，新增 `prose_arguments_accept_text_that_starts_with_a_dash`。

这条本身没什么技术含量，但它是这一轮目标的一个小注脚：**能发现它，是因为一直在拿自己写的工具干活**。
纯看代码不会撞上它。
