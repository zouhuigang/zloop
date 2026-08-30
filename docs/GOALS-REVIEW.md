# 多目标模块 code review（`src/goals.rs` + `zloop goal` 子命令）

> 范围：`src/goals.rs`（275 行，commit 6869e62 引入）与 `src/cli.rs` 里的 `GoalCmd` / `cmd_goal`，
> 连带 `state.rs` 的锁与 `find_root`、`log.rs` / `notes.rs` 的目录归属。
> 日期：2026-08-28。所有 finding 都在 `target/debug/zloop` 上实测过，实测命令与输出附在每条后面。

## 0. 结论先说

设计（"换车"而不是"同时加载多份"）是对的：`next` / `done` / `status` / runner / Stop hook / fd-lock 全都只认
`.zloop/state.json` 一个入口，切换只搬文件，"同一时刻只有一个目标在跑"这条不变量一行没改。

问题全部集中在**一个地方**：`park()` 是整套操作里第一个动作，而它既不在锁内、也不在校验之后。
于是"停走旧的"这一步成功之后的任何失败，都把项目留在**没有当前目标**的状态（下称 headless）。
F1 / F2 / F3 是同一条故障链的三段：怎么进去（F1、F2）、进去之后为什么爬不出来（F3）。

| # | 级别 | 一句话 | 位置 |
|---|---|---|---|
| F1 | 高 | park 在校验之前，`--id` 被拒时旧目标已经停走 → 项目 headless | `goals.rs:236` |
| F2 | 高 | park / engage 不在 state 锁内，正常并发就能把项目搞成 headless，最坏交错还会让同一目标出现两份 | `goals.rs:177,204,252` |
| F3 | 高 | headless 之后 `find_root` 认不出这个项目，从子目录连 `goal list` 都看不到停放的目标 | `state.rs:242` |
| F4 | 中 | 目标文件读不出来就被静默吞掉；当前目标损坏时连"停到一边"都做不到 | `goals.rs:88,182` |
| F5 | 中 | `zloop log` 跨目标串台：日志目录项目级，过滤只看文件名里的 todo id | `log.rs:88,220` |
| F6 | 低 | 停放的目标在 `goal list` 里显示"进行中" | `goals.rs:93` |
| F7 | 低 | headless 时 `goal list` 的图例说"▸ 是当前那个"，却没有任何一行带 ▸，也不给恢复指令 | `goals.rs:117` |
| F8 | 低 | `sanitize_id` 先 trim 后截断，超长 id 可能以 `-` 结尾 | `goals.rs:50` |
| F9 | 低 | ~~`goal rm` 接受目标文字片段却不需要确认~~ → 已做，见「F9 的处置（t17）」 | `goals.rs:258` |

另有两条高危是在对照 loopx 时才找出来的，见下半篇：**L1** 写回落到错误的目标（`--force` 换目标后，
在飞会话的 `done` 记到新目标头上，实测）、**L2** 同一条 todo 会被派给两个会话（`next` 无条件覆盖
`in_progress`，实测）。这两条和 F1/F2 同属"边界画错"，t4 一起修。

---

## F1（高）park 在校验之前：`--id` 被拒 = 旧目标已经停走

`create()` 的顺序是 **先 park，后校验 id**：

```rust
// src/goals.rs:231
pub fn create(root: &Path, text: &str, id: Option<&str>) -> Result<(Option<Row>, Row)> {
    let text = text.trim();
    if text.is_empty() { bail!("目标不能是空的"); }
    let parked_row = park(root)?;          // ← 236：旧目标已经离开 state.json
    let id = match id {
        Some(raw) => {
            let clean = sanitize_id(raw);
            if clean.is_empty() { bail!("--id {raw:?} 里没有可用字符…"); }   // ← 241：现在才拒
            if goals_dir(root).join(format!("{clean}.json")).exists() {
                bail!("id {clean:?} 已经有人用了…");                        // ← 244
            }
```

`switch()`（`goals.rs:219`）是同一形状：`park` 成功、`engage` 的 rename 失败，也没人把旧的搬回来。
park 之后没有任何回滚路径。

**失败场景（实测）**：项目有一个进行中的目标，用户想开新目标并自己起个中文 id。

```
$ zloop goal new "goal B second" --id "中文标题"
zloop: --id "中文标题" 里没有可用字符（只留 a-z 0-9 . _ -）
$ ls .zloop/            # state.json 没了
goals  state.json.lock
$ zloop status
zloop: no zloop state at …/.zloop/state.json (run `zloop init "<goal>"` first)
```

一次参数打错，`status` / `context` / `next` / runner / Stop hook 全部失效。用户看到的是"我的目标没了"。

## F2（高）锁没盖住 park / engage

`park`（`goals.rs:177`）和 `engage`（`goals.rs:204`）直接 `fs::rename`，都不进 `state::locked`；
`create` 只把最后那一次 `save` 放进锁里（`goals.rs:252`）。`ensure_idle`（`goals.rs:155`）检查的是
runner pid 和 `in_progress`，这两个都拦不住"另一个终端正在跑 `zloop done`"或 Stop hook 那一瞬。

**失败场景 A：锁超时 → headless（实测）**。任何持锁者都会让 `goal new` 在 park 之后 5 秒超时：

```
# 外部进程 flock 住 .zloop/state.json.lock（等价于另一个终端的 zloop done / runner 一轮 / Stop hook）
$ zloop goal new "goal D"
zloop: could not lock …/.zloop/state.json.lock within 5.0s
$ test -f .zloop/state.json || echo headless
headless
$ ls .zloop/goals/
g1.json  goal-keep-awake.json  goal.json      # 三个目标全停着，一个都没在跑
```

注意这条不需要任何错误输入，正常并发就能触发——比 F1 更容易碰到。

**失败场景 B：同一目标出现两份**。`state::transaction` 是 lock → load → 改 → save（`state.rs:374`）。
把 park 插到 load 和 save 之间：

1. `zloop done t1` 拿到锁，`load` 出目标 A；
2. `zloop goal new "B"` 的 `park` 把 `state.json` 重命名成 `goals/a.json`（不看锁）；
3. `zloop done t1` 的 `save` 走 tmp + rename，把**改动前后的 A** 又写回 `state.json`；
4. `goal new` 接着 `save` 新目标 B 到 `state.json`（或 `switch` 的 `engage` 撞上"当前目标还没停走"直接 bail）。

结果是 A 同时存在于 `state.json` 和 `goals/a.json`（`goal list` 会看到两行同名目标，id 相同），
而 `done` 那一轮的写回落在哪一份取决于交错——正是 loopx 写回顺序踩过的那类账目丢失。

## F3（高）headless 之后从子目录找不回项目

```rust
// src/state.rs:242
if candidate.join(STATE_DIR).join(STATE_FILE).is_file() { return candidate.to_path_buf(); }
```

`find_root` 只认 `.zloop/state.json`。F1/F2 之后这个文件不在，于是从任何子目录跑 `zloop` 都把 cwd 当 root：

```
$ cd sub/deeper
$ zloop goal list
这个项目还没有目标：`zloop init "目标"`
$ zloop goal switch goal-keep-awake
zloop: 这个项目还没有任何目标：`zloop init "目标"`
```

停放的目标就在上面两层的 `.zloop/goals/` 里，但看不见也切不回来。用户唯一的出路是 cd 到确切的项目根目录，
而错误信息恰恰在建议他 `zloop init`——照做就是在子目录里再建一个空项目，把真正的目标彻底埋掉。

## F4（中）目标文件读不出来就被静默吞掉

`row_of` 用 `.ok()?`：

```rust
// src/goals.rs:88
fn row_of(path: &Path, current: bool) -> Option<Row> {
    let st = state::load(path).ok()?;      // 损坏 / 版本不匹配 → 整行消失
```

于是一个损坏或版本不匹配的停放目标不出现在 `goal list`，`resolve` 回答"没有目标匹配"，
`taken()`（`goals.rs:66`）也数不到它——用户以为目标丢了，实际文件还在。

另一半在 `park`：它要求 `state::load(&cur)?` 成功才能停走（`goals.rs:182`，只为了读一个 id）。
所以当前目标一旦损坏，`goal new` / `goal switch` 全部报错：

```
$ printf '{"version":1,"goal":' > .zloop/state.json
$ zloop goal new "goal C"
zloop: corrupt state file …/.zloop/state.json: EOF while parsing a value at line 2 column 0
$ zloop goal list
这个项目还没有目标：`zloop init "目标"`
```

多目标本该提供的那条逃生路——"把坏的停到一边，开个干净的接着干"——正好走不通，
剩下的选择只有 `init --force`（归档且切不回来）或者手动搬文件。

## F5（中）`zloop log` 跨目标串台

日志目录是项目级的（`log.rs:88` → `.zloop/log/`），`entries()`（`log.rs:220`）的 todo 过滤只看文件名：

```rust
// src/log.rs:232
Some(id) => p.file_name().map(|n| n.to_string_lossy().contains(&format!("-{id}-")))
```

每个目标的 todo id 都从 `t1` 重新开始（`next_id` 在 `default_state` 里重置），所以碰撞是常态而不是例外。

**实测**：两个目标各跑完一条 `t1`，当前目标只有 1 轮记录，`zloop log --todo t1` 列出 2 个文件：

```
$ ls .zloop/log/
20260828-203935-t1-done-2.md  20260828-203935-t1-done.md
$ zloop log --todo t1          # 当前目标只跑过一轮
  .zloop/log/20260828-203935-t1-done.md    t1 · done · 2026-08-28T20:39:35+08:00
  .zloop/log/20260828-203935-t1-done-2.md  t1 · done · 2026-08-28T20:39:35+08:00
```

别的目标的过程被当成本目标的证据摆在眼前。`zloop doc` 没有这个问题——它按 `tick.log` 的相对路径取
（`log.rs:190`），账本跟着目标走。所以修的时候只需要动 `log` 的列举侧。

`.zloop/NOTES.md`（`notes.rs:14`）同样是项目级：`zloop remember` 的经验会出现在别的目标的
`zloop context` 里。这条**可能是有意的**（跨目标复用经验正是想要的），但代码和文档里都没有交代，
和"tick 账本跟着目标走"的隔离承诺读起来是矛盾的——至少要说清楚。

## F6（低）停放的目标显示成"进行中"

`row_of` 把 `Row.status` 直接抄成 `goal.status`（`goals.rs:93`），停放不改状态。于是没人跑的目标标成"进行中"：

```
  ▸ g1               完成    1/1  08-28 20:39  goal B
    goal-keep-awake  进行中  0/1  08-28 20:38  goal A keep awake    ← 没有任何 runner 在跑它
```

"进行中"在 zloop 其他地方的含义是"可以被 next 派活"，这里的语义对不上。

## F7（低）headless 时 `goal list` 的图例说谎

`list()`（`goals.rs:117`）在 state.json 缺失时只返回停放的行，`cmd_goal` 照旧打印
"共 N 个目标 · ▸ 是当前那个"，但没有任何一行带 ▸，也不告诉用户"现在没有当前目标，用
`zloop goal switch <id>` 挑一个"。见 F1 的实测输出。

## F8（低）`sanitize_id` 先 trim 后截断

```rust
// src/goals.rs:50
let out = out.trim_matches(['-', '.']).to_string();
out.chars().take(40).collect()      // ← 截断发生在 trim 之后
```

超过 40 字符时截断可能落在 `-` 上，得到 `some-very-long-goal-id-` 这样的 id。只是观感问题
（文件名合法、碰撞检查也照常工作），但顺手可以修：截断后再 trim 一次。

## F9（低）`goal rm` 接受文字片段却不需要确认

`archive`（`goals.rs:258`）走 `resolve`，包含"目标文字包含"这一档。`zloop goal rm 优化` 在
只命中一个目标时直接搬走，事后才打印搬了谁。文件还在 `.zloop/archive/`，所以不是数据丢失，
但归档动作没有 dry-run 也没有确认，和 `switch` 的模糊匹配是两种风险等级。

---

## 修复方向（留给 t4）

1. **一个事务盖住整段搬家**（治 F1 + F2）：把"校验 → park → engage/create"整体放进
   `state::locked` 里，且校验全部前移到 park 之前；park 之后的任何 `Err` 都要把文件搬回
   `state.json`（rename 的反向操作，同一文件系统内是原子的）。
2. **`find_root` 认 `.zloop/` 目录**（治 F3）：state.json 不在但 `.zloop/goals/` 有东西时也算项目根，
   并让错误信息说"这个项目有 N 个停放的目标，`zloop goal switch <id>` 挑一个"，而不是建议 `init`。
3. **`park` 不解析 JSON 也能搬**（治 F4 上半）：读不出来就用一个兜底 id（时间戳）搬走；
   `goal list` 对读不出来的文件打一行"损坏"而不是隐藏（治 F4 下半）。
4. **`log` 按目标过滤**（治 F5）：列举时和当前 state 的 `tick.log` 集合求交，或者日志文件名带 goal id。
   `NOTES.md` 的跨目标语义在 README 里写明。
5. F6 / F7 / F8 / F9 顺手改，都是几行。

---

# 对照 loopx：同一个问题它是怎么解的

> 依据：本机 `loopx 0.5.2` 源码（`site-packages/loopx`）。行号是该版本的实际行号。
> loopx 的整体形态与取舍见 `docs/loopx-scheduling-notes.md`；这里只看**多目标 / 并发 / 一致性**这一块。

## 根本差别：loopx 没有"当前目标"这个位置

loopx 的 `.loopx/registry.json` 里 `goals` 是一个**数组**，每条命令都带 `--goal-id`
（`registry.py:36 find_registry_goal`），没有"哪个是当前"的概念。zloop 反过来：只有一个槽位
（`.zloop/state.json`），切换靠搬文件。

这个取舍本身是对的——zloop 的 `next` / `done` / runner / Stop hook 因此一行都不用改（`goals.rs:1-9` 的注释说清了）。
但它引入了一个 loopx 结构上不可能有的状态：**槽位是空的**。F1/F2/F3 三条高危全部出自这里。
loopx 的 goal 数组不会"空一个格子出来"，它的等价风险是 registry 自身写坏，而那条它是拿锁 + 备份 + 健康检查兜住的（见 L3/L4）。

## L1（高）写回落到错误的目标 —— 实测

zloop 的 `done` 只认 todo id，落在**当次读到的 state.json** 上（`cli.rs:641` 一带）；
`in_progress`（`state.rs:178`）记了 `todo / round / via / host / session`，**唯独没记 goal id**。
于是「A 会话正在做目标X 的 t1」这件事，和「state.json 现在装的是哪个目标」之间没有任何绑定。

loopx 的每一次 todo 变更都由 `(goal_id, todo_id, owner)` 三元组定址：

```python
# control_plane/work_items/task_lease.py:105
def task_lease_path(*, runtime_root: Path, goal_id: str, todo_id: str) -> Path: ...
# :239  失配时直接拒绝
{"reason": "lease_owner_mismatch", "lease_owner": owner, "expires_at": lease.get("expires_at")}
```

**实测**（两个目标 X / Y，各有一条 t1）：

```
$ env CLAUDE_CODE_SESSION_ID=sess-A zloop next     # A 领到目标X 的 t1
goal: 目标X 重构缓存 | todo: X的活
$ zloop goal switch "写文档" --force               # 另一个终端切到目标Y
当前目标 [g2] 目标Y 写文档 · 进行中 0/1
$ env CLAUDE_CODE_SESSION_ID=sess-A zloop done t1 --note "X 的缓存重构做完了" --approach "X 的思路"
t1 done: X 的缓存重构做完了

当前目标: 目标Y 写文档
它的 t1: Y的活 | status: done | note: X 的缓存重构做完了     ← Y 的活被 X 的成果标成完成
它的 ticks: [('done', 'X 的缓存重构做完了', 'sess-A')]
停放: 目标X 重构缓存 | t1: open | ticks: 0                   ← X 的账本一条记录都没有
```

X 干的活记在 Y 头上，X 自己看起来一轮没跑，`--approach` 留下的技术文档也归档进了 Y 的日志。
这是 `--force` 换目标的默认代价，而 `--force` 的提示只说"会让 runner 中途换活"（`goals.rs:160`），
没说"已经派出去的活写回时会串目标"。

**修法很便宜**：`InProgress` 加一个 `goal: String`，`done` 发现 `st.goal.id != ip.goal` 就拒绝并告诉用户
`zloop goal switch <原目标>` 再写回。一个字段 + 一个判断。

## L2（高）没有 lease，同一条 todo 会被派给两个会话 —— 实测

`cmd_next`（`cli.rs:533`）无条件覆盖 `in_progress`；`tick::decide`（`tick.rs:92`）通篇不看 `in_progress`；
`policy.stale_after_min` 只被 `phase.rs:80` 用来在文案里打一个 ⚠，**`next` 自己不用它**。
所以"这条活已经有人拿着"根本不是一个会影响决策的状态。

```
$ env CLAUDE_CODE_SESSION_ID=aaaa-session-A zloop next     # A 领到 t1
$ env CLAUDE_CODE_SESSION_ID=bbbb-session-B zloop next     # B 也领到 t1
in_progress = {'todo': 't1', 'started_at': '…20:46:07', 'round': 1, 'via': 'next', 'session': 'bbbb-session-B'}
                                ↑ 还是 A 的开始时间和轮次，session 已经被 B 顶掉
```

第二次 `done` 会被 "t1 is already done" 挡住，所以账目没重复；真正的损失是**两个 agent 同时在改同一批文件**，
以及 A 的在飞状态从此不受保护——`ensure_idle`（`goals.rs:155`）报的是 B 的持有者，B 一退出，
`goal switch --force` 看起来就是安全的。

loopx 在这件事上做了三层，zloop 一层都没有：

| loopx | 位置 | 作用 |
|---|---|---|
| `normalize_owner` + `claimed_by` | `task_lease.py:68`、todo 元数据 | 每条 todo 记谁拿着 |
| `ttl_seconds` / `expires_at` | `task_lease.py:75` | 租约会过期，不需要人来清 |
| `hard_lease` handoff mode | `task_lease.py:181,231-244` | 换持有者必须先拿到租约，失配报 `lease_owner_mismatch` |
| `normalize_idempotency_key` / `completion_turn_key` | `task_lease.py:58` | 重复写回幂等 |

zloop 不需要多 agent 协作那一整套，但"派活时看一眼有没有人拿着、超过 `stale_after_min` 才允许重派"
是两行代码，而且 `stale_after_min` 这个字段已经存在了——现在它只是个装饰。

## L3（中）锁超时不说是谁持锁

zloop 的 `state::locked`（`state.rs:338`）：固定 5 秒、失败只有一句
`could not lock …/state.json.lock within 5.0s`。F2 的 headless 就是这条超时触发的，
而用户拿到这句话之后无从判断该等谁、还是有个卡死的进程要杀。

loopx 的 `file_lock.py` 在同一个位置多做了四件事：

1. **锁文件里写持有者**：`_holder_record`（`file_lock.py:157`）把 `pid / agent_id / operation / acquired_at`
   json.dump 进锁文件本身，释放时补 `released_at`；
2. **超时错误带持有者和处置步骤**：`_timeout_error`（`file_lock.py:334`）读出 holder，附
   `operator_action`：先看 pid 和 operation、确认真卡死再杀、**不要删锁文件（内核锁才是权威）**；
3. **超时留档**：`_append_incident` 把每次超时追加到 `<file>.lock.incidents.jsonl`（O_APPEND + fsync）；
4. **三档策略**（`LOCK_POLICIES`，`file_lock.py:86`）：`MUTATION` 5s / `MONITOR` 1s /
   `SINGLE_FLIGHT` 试一次就走（`try_exclusive_file_lock`）。

对 zloop 的意义很直接：`zloop status` / `context` / `log` 这些只读命令现在也要等满 5 秒才报错，
而它们完全可以是 MONITOR 那一档；写命令的超时信息应该告诉用户"被 pid 12345 的 runner-round 持着"——
zloop 已经有 `.zloop/runner/pid` 这套手法（`daemon.rs:17`），把它挪到锁文件上是同一件事。

## L4（中）清单坏了要报，不能藏

zloop 的 `row_of` 用 `.ok()?` 静默丢行（F4），headless 时 `goal list` 干脆说"这个项目还没有目标"（F7）。
loopx 在这一层是专门写了健康检查的：

```python
# control_plane/goals/global_registry_health.py:34  collect_global_registry_health
kind="source_registry_missing"   message=f"`{goal_id}` source registry is missing"
kind="stale_source_registry"
kind="state_file_missing"        message=f"`{goal_id}` active state file is missing"
message=f"current registry excludes {len(missing_from_current)} global goal(s)"
# 每条 finding 都带 orphan_retirement_action：下一步该跑什么命令
```

外加 `registry.py:310 inspect_registry` 的 `add_problem` 逐项报错、`registry_writability.py`
判断能不能写、`global_registry.py:202 route_collision` + `collision_message` 检测两个 goal 指向同一目标。
**F2 最坏交错产生的"同一个 id 出现两份"正是 route_collision 那一类**，zloop 现在没有任何检测。

zloop 不需要 health 子系统，需要的是三行：读不出来的目标文件在 `goal list` 里打一行"损坏（文件路径）"；
没有当前目标时明说"当前没有目标，N 个停着，`zloop goal switch <id>`"。

**处置（2026-08-29，[#4](https://github.com/zouhuigang/zloop/issues/4)）**：那"三行"F4/F7 早就补进 `goal list` 了，
但补完才看清 `goal list` 治不了的那一半——**不报错的不一致**。所以另加了一条只读命令
`zloop doctor`（`src/doctor.rs`，约 260 行，11 个回归测试）：照抄 `collect_global_registry_health` 的
**形状**（逐条 finding + 每条带"下一步跑什么"），但不引入 health 子系统，就是一个函数扫一遍文件。

11 类检查，分两档：`headless` / `broken_goal` / `id_filename_mismatch` / `duplicate_goal_id`（= loopx 的
`route_collision`，F2 最坏交错的产物）/ `dangling_in_progress` / `dangling_blocked_by` / `duplicate_todo_id` /
`next_id_reuse` 算"要修"（退出码 1）；`missing_log` / `broken_archive` / `archive_id_collision` /
`stale_pid` 算"留意"（退出码 0，免得 CI 里一个被删掉的旧日志就红一片）。

两个当时没想到、写测试才落定的点：

1. **`dangling_blocked_by` 是 `zloop compact` 自己造出来的**，不是只有手改文件才会有：compact 把做完的
   todo 搬进 `archive/compact-*.json`，而依赖它的那条 todo 的 `blocked_by` 还指着它，`is_executable`
   要求依赖"存在且 done"（`todo.rs:161`）——于是这条 todo 从此永远排不上，一声不吭。回归测试就是
   照这条真实路径走的（`tests/doctor_test.rs::compacted_dependency_leaves_a_todo_that_can_never_run`）。
   **（t39 之后不再成立）**：`compact` 现在会把还有人等的那条留在清单里，所以这个坏状态只剩手改
   `state.json` 和老版本留下的文件两个来源；检查照旧要有，测试改名为
   `a_dependency_that_is_not_in_the_list_leaves_a_todo_that_can_never_run`，造状态的手法换成直接改文件。
2. **只读得是硬约束，而且要用测试钉住**：doctor 不能调 `daemon::running()`——它会顺手删掉过期的 pid
   文件。`stale_pid_is_reported_and_doctor_changes_nothing` 同时验两件事：doctor 跑完 pid 文件还在、
   `state.json` 字节不变；再跑一次 `zloop status` 证明"会清它的是 status，不是 doctor"。

## L5（关键手法可以直接抄）mutate = 锁内 load + 备份 + 写

loopx 的全局 registry 事务（`global_registry.py:57`）：

```python
def mutate_global_registry(global_path, operation, reducer):
    with exclusive_file_lock(global_path, operation=operation):   # ← 锁在最外层
        current = load_registry(global_path)                      # ← load 在锁内
        reduction = reducer(copy.deepcopy(current))
        if wrote and reduction.backup_label and global_path.exists():
            write_json(_global_registry_backup_path(...), current) # ← 写前先备份
        if wrote:
            write_json(global_path, reduction.payload)
    return {"before": current, "after": reduction.payload, "receipt": …, "backup_path": …}
```

对比 zloop 的 `create`（`goals.rs:231`）：park 在锁外、校验在 park 之后、锁只盖住最后那次 `save`、没有备份、
失败不回滚。**t4 要做的就是把 `park + engage/create` 变成这个形状**：校验 → 取锁 → 锁内搬 → 失败搬回。
zloop 的优势是搬家用 `fs::rename`（同一文件系统内原子），连备份都不必——反向 rename 就是回滚。

## L6（设计取舍，写进文档而不是修）跨项目视图

loopx 把项目 registry 同步到 `~/.codex/loopx/registry.global.json`（`global_registry.py:623`
`sync_project_registry_to_global`，842 行的模块）并配 4 个 global skill（`loopx-global-todos/gates/risks/summary`）。
zloop 明确不要全局 registry（`docs/DESIGN.md` 的 G1），代价是：**`zloop goals` 只看得见当前项目**，
多个项目各自停放的目标只能 cd 进去一个个看。

这不是 bug，但 README 里应该写明。真要补，最小形态是 `~/.zloop/projects.jsonl` 只记
`{root, last_seen}`（一行一个项目，`init` / `goal new` 时追加），`zloop goals --all` 现场去各项目读
state——不同步任何目标内容，也就没有 loopx 那套 merge / collision / retirement 的负担。

**处置（2026-08-29，[#8](https://github.com/zouhuigang/zloop/issues/8)）**：按"写进文档而不是修"办了——README 的
[6.2 一个项目多个目标](../README.md#62-一个项目多个目标) 末尾新增两节：边界（含父目录里 `zloop goals` 的实测输出、
不做全局 registry 的三条理由、今天可用的那行 shell）＋最小形态（6 行决策表 + 约 100 行改动量估计 + 什么时候才做）。
代码一行没动。

## 反过来说：zloop 在这一块比 loopx 好的地方（别改坏）

1. **JSON 是唯一真源**。loopx 的 todo 元数据塞在 Markdown 的 HTML 注释里且做 URL 编码
   （`note=%E6%96%B0…`），人读不懂、机器要三层解析。zloop 的 `state.json` 人机同源。
2. **切换是一次 rename**，不需要 registry 一致性维护，也就没有 `merge_goal_entries` /
   `route_collision` / `retire_global_registry_goals` 这三个模块要伺候。
3. **一屏能看完**。`zloop goal list` 一条命令；loopx 光把一个目标 bootstrap 起来就是 8 步事务、12 条 CLI 调用。

F1–F9 全都是实现层面的疏漏，不是这个设计的必然代价——这一点从 loopx 的 `mutate_global_registry`
可以看得很清楚：它的事务边界画对了，zloop 只是把 park 漏在了边界外面。

---

# 修复记录（t4）

> 全部改动在 `src/state.rs` / `src/goals.rs` / `src/tick.rs` / `src/cli.rs`，
> 加 5 个回归测试（`tests/cli_test.rs` 末尾）。`cargo test`：76 passed / 0 failed。

## 改了什么

| # | 修法 | 位置 |
|---|---|---|
| F1 | `create` / `switch` **先校验完再动文件**：不合法的 `--id`、撞了的 id、runner 在跑、悬空轮次，全部在 park 之前拒掉 | `goals.rs create/switch` |
| F2 | 整段搬家进 `state::locked`（`LOCK_WAIT` 5s），park 之后任何失败都 `unpark` 搬回来 | `goals.rs park/unpark/create/switch` |
| F3 | `find_root` 认 `.zloop/goals/`；`state::load` 缺文件时区分"没初始化"和"目标全停着"，后者直接给 `goal switch` | `state.rs find_root/parked_count/load` |
| F4 | `row_of` 对读不出来的文件返回一行 `status=broken` 而不是 `None`；`park` 不解析 JSON 也能搬；`archive` 不再要求能 load | `goals.rs row_of/park/archive` |
| F6 | 清单里停着的 active 目标显示"停放"，坏的显示"损坏" | `cli.rs row_status_zh` |
| F7 | 没有当前目标时图例改成"当前没有目标在开着"并多印一行 `goal switch <id>` | `cli.rs cmd_goal::List` |
| F8 | `sanitize_id` 改成先截断再 trim | `goals.rs sanitize_id` |
| L1 | `done` 写回前查一遍"这条活是不是停放中的目标派给本会话的"，命中就拒绝并指出该切回哪个目标；`--force` 可硬记。`goal switch/new --force` 停走带在飞派活的目标时当场打 ⚠ | `goals.rs parked_holder`、`cli.rs cmd_done/warn_parked_handout` |
| L2 | `next` 撞上别的会话的派活时返回 `held_by_other`（带持有者与开始时间），不抢占、不记 tick、不清 `in_progress`；`policy.stale_after_min` 到点后照旧可以重派，设 0 关掉 | `tick.rs held_by_other/hold_decision`、`cli.rs cmd_next` |

顺手修掉一个**改的过程中新引入**的问题：当前目标读不出来时 `taken()` 数不到它，`create` 又在 park 之前定 id，
于是停走的那份和新目标都拿到 `g1`。改成 **id 在 park 之后再定**（`--id` 仍然在 park 前做早期校验，park 后再查一次撞车并可回滚）。

## 怎么验的

`cargo test` 之外，每条都在 scratch 项目上按 t1/t2 里那份实测脚本重跑一遍：

| 场景 | 修前 | 修后 |
|---|---|---|
| `goal new --id "中文标题"` | state.json 消失，`status` 报 no zloop state | 报错退出，`state.json` 还在，`status` 照常显示原目标 |
| `goal new --id <当前目标的 id>` | （旧版不检查当前 id，会撞车） | `id "…" 已经有人用了`，什么都没动 |
| 外部进程 flock 持锁时 `goal new` | 5 秒后超时，state.json 已经被停走 → headless | 超时报错，`state.json` 还在 |
| state.json 写坏后 `goal new` | `corrupt state file`，切不走也开不了新的 | 坏的那份停到 `goals/g1.json`（list 显示"损坏"），新目标拿 `g2`，`goal rm g1` 清得掉 |
| headless 后从子目录 `goal list` | "这个项目还没有目标" | 列出 2 个停放目标 + `zloop goal switch <id>`；`status` 的报错也指这条路 |
| 两个会话各跑 `next` | 都拿到 t1，`in_progress.session` 静默变成后来那个 | 第二个报 `held_by_other`（说明在 sess-A 手里、第 1 轮、几点开始），持有者不变，不记 tick |
| `--force` 换目标后原会话 `done` | 成果记到新目标，原目标账本一条不留 | 被拒并指出 `zloop goal switch g1`；新目标 t1 仍 open、ticks 仍为 0；切回去再写回，落在正确的目标上 |

## 还没修

- ~~**F5（`zloop log` 跨目标串台）**~~ → 已修，见下面「F5 的修法」。
- ~~**F9（`goal rm` 靠文字片段匹配却不需要确认）**~~ → 已做，见下面「F9 的处置（t17）」。
- ~~**L4（缺健康检查）**~~ → 已做，见下面 L4 的处置记录（`zloop doctor`）。
- ~~**L3（锁超时不说是谁持锁）**~~ → 已做，见下面「L3 的处置（t16）」。
- **L6（跨项目视图）**："可以更好"，不是错，处置是"写进文档而不是修"。

---

# 对照 Warp 的自我改进 agent：回路缺哪几段

> 依据：[How Warp builds self-improving agents on Claude](https://claude.com/blog/how-warp-builds-self-improving-agents-on-claude)。
> 这一节问的不是"多目标有没有 bug"，而是"zloop 攒下来的证据和文档，有没有让下一轮变得更好"。

## Warp 的机制，六句话

1. **两层 skill**：inner/base skill 装领域知识（比如"PR 打开时怎么做 code review"）；outer/improver skill 是**按计划跑的观察者**，不是每个任务都跑。
2. improver 的动作："pulls the accumulated human feedback, compares what the agent suggested against how humans responded, and proposes a **small, focused edit** to the base skill."
3. **反馈在人本来就在的地方收**（PR 和 issue 上），没有额外的提交步骤。"Low friction is what keeps signal flowing. If you make it too hard you're not going to get the feedback."
4. **skill 是纯文件**，所以能走正常的 PR review / approve / merge；merge 之后"the next agent run inherits improvements"。
5. 指标用**人本来就在看的那些**：time to merge、contributor count、cost，喂回 improver。
6. 人始终在环：改进是提案，人审、批、合，下一轮才继承。

## zloop 已经有的那半条回路（先承认）

`zloop remember "<一句话>"` → `.zloop/NOTES.md`（`notes.rs`）→ `zloop context` 的「经验」节（`context.rs:99`）→ 下一轮 agent 读到。
这就是 Warp 第 3、4 条的雏形：**低摩擦**（一条命令，在干活的地方留）、**纯文件**（Markdown，人能读能改）、**下一轮继承**。
`done --pitfall` 让"踩过的坑"在写回时顺手留下；`policy.require_doc` 甚至比 Warp 严——它直接卡住 `done`，没有 `--approach` 就不让完成。

缺的是后半条：**攒下来的东西没有人去读、去整理、去变成对下一轮的修改。**

| # | 缺口 | 现状位置 | 最小补法 |
|---|---|---|---|
| W1 | 没有"人怎么回应"的通道 | `tick` 里全是 agent 自述 | `zloop feedback <todo> "<人说的>"` |
| W2 | 经验只有尾部 5 条会被读到，从不整理 | `notes.rs` append-only + `context.rs:99` | 一个 reflect 动作，重写而不是只追加 |
| W3 | skill 是只写不读的，用户的改进被静默覆盖（实测） | `hosts.rs:67 write_managed` | 留一个用户区块，install 时保留 |
| W4 | 失败信号的落点是"停下来"，不是"学到" | `tick.rs decide` + `cli.rs cmd_done` | fail 轮次强制 `--pitfall`；context 列失败 |
| W5 | 有 documented 布尔，没有质量/趋势指标 | `log.rs:41` `is_complete()` | `zloop stats` |
| W6 | 没有"定时的观察者"，runner 只干活 | `runner.rs` 的每轮循环 | `--reflect-every N` |

## W1（关键缺口）没有"人怎么回应"的通道

improver 的输入是 **agent 建议了什么** 和 **人最后怎么回应** 之间的**差**。zloop 里这个差算不出来：
`tick` 的 `note` / `approach` / `decision` / `pitfall` / `evidence` **全部由 agent 自己写**，`--block` 是 agent 反过来问人。
用户说"这条做得不对，重做"、"这个方向别走了"——这句话在状态里**没有位置**，说完就没了。

没有这个通道，W2 / W6 那个 improver 就没有信号可读；Warp 整套机制是架在它上面的。

最小形态：`zloop feedback <todo> "<人说的>"`，记成 `outcome=feedback` 的 tick（`tick::OUTCOMES` 现在 6 个，加第 7 个），
`context` 里单列一节「用户对上一轮的反馈」，`doc` 里和 agent 的自述并排放。反馈存在 `state.json` 里，所以**跟着目标走**，
多目标下天然隔离——这一点刚好被前半篇的 L1 反证过：任何没绑定目标的写回都会串。

## W2 经验只有尾部 5 条会被读到，而且从不整理

`notes.rs` 只追加（`remember` 里是 `OpenOptions::new().append(true)`），`context.rs:99` 只取 `notes::recent(root, 5)`。
没有去重、没有分类、没有"这条已经过时"。写第 20 条经验时，第 1–15 条对 agent 来说等于不存在。

Warp 的 improver 干的正是这件事：把**累积的**反馈压缩成**一次小编辑**，而不是把原始反馈全塞给下一轮。zloop 缺"整理"这个动作。

最小形态：一个 reflect 动作（命令或 runner 间隔，见 W6），读全量 NOTES + 最近的失败 + 用户反馈，
输出"建议保留 / 合并 / 删除"的清单让人点头，**重写** NOTES.md 而不是继续追加。

## W3 skill 是只写不读的，用户的改进被静默覆盖 —— 实测

```rust
// src/hosts.rs:67
fn write_managed(path: &Path, content: &str) -> Result<bool> {
    if path.exists() {
        let current = fs::read_to_string(path)?;
        if !current.contains(MANAGED_MARK) && … {
            bail!("{} exists and is not managed by zloop; remove it first", …);
        }
        …
    }
    fs::write(path, content)?;   // ← 带 managed 标记的（= zloop 自己装的每一份）无条件重写
```

只有**不带** `<!-- zloop-managed:v1 -->` 标记的文件才被保护。zloop 自己装的 SKILL.md 一定带这个标记，
于是用户/agent 对它的任何改进，在下一次 `zloop install`（比如升级之后）时消失，输出只说 `wrote`：

```
$ zloop install --claude                       # 干净安装
wrote  …/.claude/skills/zloop/SKILL.md
$ printf '\n## 我自己的补充\n- 这个项目里 done 之前一定要跑 cargo test\n' >> …/SKILL.md
$ grep -c "我自己的补充" …/SKILL.md
1
$ zloop install --claude                       # 升级后再装一次
wrote  …/.claude/skills/zloop/SKILL.md
$ grep -c "我自己的补充" …/SKILL.md
0                                              ← 静默覆盖，没有任何提示
```

这正好和 Warp 第 4 条反着：那边 skill 是**改进的载体**（改了、审了、合了，下一轮继承），
这边 skill 是**模板的投影**（只能被 zloop 写，人写的会被冲掉）。

最小形态：SKILL.md 末尾留一个 `<!-- zloop:user -->` 之后的自由区块，install 时原样保留；
或者发现 managed 区域之外有改动就打印 diff 并要求 `--force`。

## W4 失败信号的落点是"停下来"，不是"学到"

`fail_streak >= 3` → `Decision::stop("fail_streak")`，`progress_streak >= 8` → stop（`tick.rs decide`）。**停是对的**，
但那几次失败的**原因**没有结构化落点：

- `--outcome fail` 不强制 `--pitfall`——`cmd_done` 里 `finishing = outcome == "done" && block.is_none()`，
  文档要求只卡"完成"，失败的那一轮可以只留一句自由文本 note；
- `context` 只给最近 3 次 tick 的一行摘要（`context.rs:43`），不专门列失败原因；
- `stale`/streak 计数会因为下一次 `done` 归零，失败记录就沉到 `ticks` 数组深处，没人再看。

结果是同一个坑第二次踩在 zloop 里完全可能——而这正是 Warp 那套机制存在的理由。

最小形态：fail 轮次强制 `--pitfall`（和 done 强制 `--approach` 是同一个机制，policy 里加一个字段就行）；
`context` 加一节「本目标已经失败过的地方」，从 fail / block tick 的 pitfall 里抽。

## W5 有 documented 布尔，没有质量或趋势指标

```rust
// src/log.rs:41
pub fn is_complete(&self) -> bool {
    self.approach.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false)
}
// src/log.rs:161  判定日志文件里有没有那一节
fs::read_to_string(path).map(|s| s.contains("\n## 实现思路\n")).unwrap_or(false)
```

`tick.documented` 回答的是"**交了没交**"，不是"**好不好**"。而 Warp 那套"人本来就在看的指标"的原料 zloop 全都有：
每条 tick 上的 `cost_usd` / `duration_ms` / `num_turns`，每条 todo 花了几轮，progress 与 done 的比例，
被 `--block` 打断几次，`acceptance` 写了没写。缺的只是把它们汇成一屏。

最小形态：`zloop stats` —— 每条 todo 的轮次 / 花费 / 是否一次过，**返工率**（progress 轮数 ÷ done 轮数），
被 block 次数，无文档轮次数。这一屏同时就是 W2 那个 reflect 动作的输入。

## W6 没有"定时的观察者"，runner 只干活

Warp 特意说明 improver **不是每个任务都跑**，而是按计划跑——反思要攒够信号再做。
zloop 的 runner 每轮只做"取一条 → 干 → 写回"（`runner.rs`），没有任何一轮是用来回看的。

最小形态：runner 加 `--reflect-every N`（默认关），每 N 轮插一轮**不改代码**的 reflect：
读最近 N 轮日志 + 失败原因 + 用户反馈 → 提一条对 todo 清单 / policy / NOTES 的修改建议 → 用 `--block` 交给人。
这不是新子系统，是现有循环的一个可选间隔。

## 什么不该抄

- **不要抄"跨人聚合反馈"**。Warp 的规模是 10M Claude Code sessions（400K+ 每周）、40M Warp Agent 对话、
  数百贡献者、数千次 code review，反馈能靠数量变成统计显著的信号。zloop 是单人单项目，`.zloop/` 还被
  gitignore（有意的：per-machine 状态）。zloop 该做的是**一条高质量反馈立刻生效**——恰好也是 Warp 自己那句
  "A small amount of detailed, domain-specific feedback from a senior engineer can be worth more than lots of cursory feedback"。
- **不要为 improver 引入第二套调度**。Warp 的 improver 跑在他们的 Oz 平台上；zloop 已经有 runner 和 policy，
  reflect 应该是 runner 的一个间隔 + 一条命令，不是新的守护进程。
- **不要让 improver 自动改 SKILL.md 或 policy**。Warp 那边是 PR review 之后 merge；zloop 没有 PR，
  所以"人审"的形态就是 `--block` 或一条待办，人点头才落地。这条和 zloop 现有的 gate 机制是一致的。

## 排优先级

1. **W1 反馈通道** —— W2/W6 都依赖它，而且最便宜（一个 outcome + context 一节 + 一条命令）。
2. **W4 失败强制留坑 + context 列失败** —— policy 一个字段 + context 一节，直接减少重复踩坑。
3. **W3 install 别覆盖用户区块** —— 唯一一条已实测的数据丢失。
4. **W5 `zloop stats`** —— 原料齐了，只是没人汇总。
5. **W2 / W6 reflect 动作与间隔** —— 建在 1/2/4 的数据上，最后做。

前四条加起来大概是"一个 outcome + 一个 policy 字段 + context 两节 + 一个只读命令 + install 保留区块"，
没有一条需要新概念——这也是判断该不该抄的标准：**Warp 的回路值得抄，Warp 的规模不值得抄**。

---

# F5 的修法（t7）

## 思路：归属的权威来源是 tick，不是文件名

`.zloop/log/` 是项目级目录，而每个目标的 todo id 都从 `t1` 重新开始，所以**文件名里没有任何能定目标的信息**。
但 `tick.log` 有——它跟着目标的 state 文件走，`zloop doc` 一直用的就是它（这也是为什么 doc 从来没串过）。
所以 `log` 的列举侧改成认账本：

```rust
// src/log.rs  entries()
let mine   = state.ticks 里所有 tick.log;                              // 当前目标 → 列
let theirs = .zloop/goals/*.json + .zloop/archive/*.json 里所有 tick.log; // 别的目标 → 不列，计入 hidden
// 两边都没提到的 → 列
```

归档目录也要扫：`init --force` 和 `goal rm` 把整份 state 搬进 `.zloop/archive/`，那些轮次同样不是当前目标的。
`zloop compact` 写的 `compact-*.json` 不是一份完整 state（只有 `todos` / `ticks` 两个数组），
`state::load` 会因为缺 `version` / `goal` 解析失败，正好被 `.ok()` 跳过——于是 compact 掉的轮次落在"无主"那一档，
按下面的理由继续列出来。不需要为它写任何文件名判断。

**为什么无主文件选择"列"而不是"不列"**：宁可多列一两份，也不要把用户自己的历史藏起来。
`zloop compact` 会把老 tick 搬进 `.zloop/archive/`，那些轮次的日志文件就此无主；如果按"不在账本里就不列"处理，
一个长期项目 compact 之后会突然看不见自己的过去。

**为什么不按目标 id 分子目录**：`park` 在 id 撞车时会给目标换一个 id（`goals.rs park`），
子目录名就跟着失配，那份日志会整批消失。tick 路径不受 id 变更影响。

## 顺手修掉的一个相邻 bug

`cmd_log` 判断"这一轮该不该有实现思路"用的是 `name.ends_with("-done.md")`——
而重名时 `log::write` 会加后缀，变成 `…-t1-done-2.md`，于是**跨目标同秒同 todo 产生的日志永远不会被标 ⚠**。
现在改成认 tick 的 `outcome` / `documented`；只有无主文件才退回文件名，并且 `name_is_done()` 会先剥掉 `-2` / `-3` 这类后缀。

## 实测

```
$ ls .zloop/log/                      # 两个目标各一轮，同一秒，都叫 t1
20260828-213510-t1-done-2.md  20260828-213510-t1-done.md

$ zloop log                           # 当前是目标B
  .zloop/log/20260828-213510-t1-done-2.md  t1 · done · 2026-08-28T21:35:10+08:00
另有 1 份日志属于别的目标，没有列出来（`zloop goal list`）

$ zloop log --todo t1                 # 修前这里会列出 2 个
  .zloop/log/20260828-213510-t1-done-2.md  t1 · done · 2026-08-28T21:35:10+08:00
另有 1 份日志属于别的目标，没有列出来（`zloop goal list`）

$ zloop goal switch 目标A && zloop log
  .zloop/log/20260828-213510-t1-done.md    t1 · done · 2026-08-28T21:35:10+08:00

$ zloop compact --keep-days 0 && zloop log     # tick 归档后日志无主
  .zloop/log/20260828-213510-t1-done.md    t1 · done · 2026-08-28T21:35:10+08:00   ← 仍然列出
```

无主 + 该有文档但没有的情况（`--no-doc` 的 `-done-2.md` 被 compact 掉）也验过，⚠ 正常打上——
这正是旧的 `ends_with("-done.md")` 漏掉的那一格。

本仓库实机复验（历史比较厚，最能说明问题）：

```
$ ls .zloop/log/*.md | wc -l            40
$ zloop log
  … 当前目标的 4 轮 …
另有 36 份日志属于别的目标，没有列出来（`zloop goal list`）
```

40 = 4（当前目标）+ 15（两个停放目标）+ 21（早期被 `init --force` 归档的目标）。修前这 36 份会混在列表里，
其中 21 份还因为写在 `require_doc` 之前而全部带 ⚠——等于把别人的欠账记在当前目标头上。

回归测试：`log_lists_only_the_current_goals_rounds`（`tests/cli_test.rs`）覆盖上面 5 种情况；
`init_force_archives_the_previous_goal` 的断言也跟着改了——它原来用 `entries()` 非空来证明"日志没被删"，
而 `entries` 的语义现在是"当前目标的轮次"，所以改成直接查目录里文件还在、并断言它们计入 `hidden`。
`cargo test`：77 passed / 0 failed。

---

# W1 的实现（t8）

`zloop feedback <todo> "<人说的>"` —— 第 7 个 outcome，`tick.rs:OUTCOMES` 从 6 个变 7 个。

## 为什么值得单独一条命令

`note` / `approach` / `decision` / `pitfall` / `evidence` **全是 agent 自述**，`--block` 是 agent 反过来问人。
"人怎么回应"在旧状态里没有位置，于是 Warp improver 读的那个差（agent 建议的 vs 人接受的）在 zloop 里根本算不出来。
加一路人写的信号，是后面所有改进动作（W2 的整理、W6 的定时反思）的前提。

## 语义（都是刻意选的）

| 行为 | 选择 | 理由 |
|---|---|---|
| 计入配额 / 推进轮次 | **不** | `COUNTED` 只含 done/progress/fail；说句话不该吃掉一轮预算 |
| 打断 fail / noop / progress streak | **会** | 循环停下来"等人"，人开口说话正是它该等到的东西。实测 3 次 fail → `WAIT (fail_streak)`，`feedback` 之后立刻 `RUN` |
| 改 todo 状态 | **不** | 反馈是信号不是写回；要重做仍然是 `zloop edit <id> --status open`（命令输出会在 todo 已完成时主动提示这条路） |
| 碰 `in_progress` | **不** | 可能正有会话在飞，反馈不能把它的在飞状态清掉 |
| 存哪 | `state.json` 的 ticks | 于是**跟着目标走**，多目标之间天然不串——L1 已经反证过：任何不绑定目标的写回都会串 |
| 交接包里显示多久 | 到下一次 done/progress 为止 | `pending_feedback()` 只取"上一轮干活之后才到的"。已处理的不再占版面，但一直留在 `zloop doc` 里 |

## 三个出口

1. **`zloop context`**：新增一节「用户对上一轮的反馈（先处理这些）」，插在「当前判断」和「下一条」之间，
   并把裁剪保护位从 3 节改成 4 节（`context.rs` 的 `protected`），所以篇幅超预算时它不会被裁掉。
   同时把 feedback 从「当前判断」里排除，避免同一句话出现两次。
2. **`zloop doc <todo>`**：`log::assemble` 原来只收 `log.is_some()` 的 tick，现在也收 feedback，
   渲染成 `### 用户反馈 · <时间>` + 引用块，**紧跟在它回应的那一轮后面**——事后翻文档能看出方向为什么变。
3. **`zloop status`**：多一行 `反馈`（黄色），免得"我说了它没反应"无从判断。

另外把每轮协议（`prompt.rs` 的 `PROTOCOL`）第 1 条改成"交接包里有反馈就先按它调整这一轮的做法，别当没看见"——
通道建好了还得让 agent 知道要看。

## 验证

`cargo test` 79 passed / 0 failed，新增两条：

- `feedback_records_what_the_human_said`：记账正确（outcome/note/round/todo 状态/in_progress）、
  context 单列且不重复、排在「下一条」之前、doc 里紧跟对应轮次、status 有提示、
  处理过之后不再占交接包、未知 todo 与空话都退出 2；
- `feedback_breaks_the_fail_streak`：3 次 fail → `WAIT (fail_streak)` → `feedback` → `RUN`，
  且 `window_ticks` 仍然只数到 3 条（不吃配额）。

## 剩下的（W2–W6）

W1 是其他几条的前提，现在它在了：W4（fail 强制留坑 + context 列失败）和 W5（`zloop stats`）可以独立做；
W2（整理经验）和 W6（`--reflect-every N`）建在 W1+W4+W5 的数据上，留到最后。

---

# 用起来才发现的四条（t9，用户实机反馈触发）

目标的 8 条 todo 全部了结之后，用户看着 `zloop status` 问了两句话，问出四个缺陷。
这一节记下来是因为它们都属于同一类：**同一份事实在一屏里有两套算法 / 两种叫法**。

## S1（高，误导决策）进度的分母把 deferred 也算进去

`cli.rs` 里 `finished` 只数 `status == "done"`，而分母是 `st.todos.len()`。
但 `todo::is_terminal` 认为 `deferred` 和 `done` 一样已了结（`todo.rs:12`），调度器据此判定"全部完成"。
于是同一屏出现三条互相矛盾的话：

```
  ✅ 完成      ████████████░░░░ 75%          ← 进度条：6 ÷ 8
  阶段    8 条待办全部完成，目标结束          ← 调度器：8 条都了结了
  7. 命令详解…  t5 ⏭ 已延后                  ← 那两条永远不会变成 done
```

用户的第一反应是"卡在 t5/t6 了，得等它们跑完"——**没卡，是分母在骗人**。
修法：`planned = total - deferred` 作为百分比和进度条的分母，计数写成 `6/6 完成 · 2 条延后`，
阶段那句也改成 `6 条待办全部完成，目标结束（另有 2 条延后）`。

## S2（高，误导决策）序号和 id 不同号，而做完的行不显示 id

清单按 `state.todos` 的数组顺序（= 执行顺序）编号，id 按创建顺序发。
`zloop done --next` 把后继插在**当前这条的后面**，于是这个目标的对应关系是：

| 步骤 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 |
|---|---|---|---|---|---|---|---|---|
| id | t1 | t2 | t3 | **t8** | **t4** | **t7** | t5 | t6 |

旧版只给**没做完**的行显示 id（注释写着"做完的那些不需要 id——需要敲命令的才需要"），
所以屏幕上只有 `t5`/`t6` 两个 id 冒出来，用户自然读成"第 5、第 6 步"。
而 id 恰恰是敲命令要用的词（`zloop done t4`、`zloop doc t7`、`zloop log --todo t7`）。

修法：清单改成四列表，**每行都带 id**：

```
  清单    6/6 完成 · 2 条延后
  ┌──────┬────┬──────────────────────────────┬──────────┐
  │ 步骤 │ id │ 这一步做什么                 │ 进展     │
  ├──────┼────┼──────────────────────────────┼──────────┤
  │    4 │ t8 │ 实现 W1：zloop feedback…     │ ✅ 完成  │
  │    7 │ t5 │ 命令详解（误挂在…）          │ ⏭ 已延后 │
  └──────┴────┴──────────────────────────────┴──────────┘
```

列名和框线是用户挑的。四列正好对上用户自己记过的一条偏好——**别把多个维度塞进一个字符串**：
旧版右栏 `t5 ⏭ 已延后` 把 id、图标、状态词挤在一格里，拆开之后每一列只答一个问题。
顺带把 ⏳ 的依赖提示从"等第 3 步"改成"等 t3"：id 现在每行都看得见，不必再绕一层序号。

## S3（中，画表格才暴露）`style::width` 把 7 个符号多算一列

旧实现是一张手写区间表，把 `0x231A..=0x23FA` 整段当成两列宽。按 Unicode East_Asian_Width 实际是：

| 符号 | 码位 | EAW | 旧 style::width | 实际 |
|---|---|---|---|---|
| ⏭ ⏱ ⏸ ⏹ ⏺ | U+23ED/23F1/23F8/23F9/23FA | N | 2 | 1 |
| ⚠ | U+26A0 | N | 2 | 1 |
| ✓ | U+2713 | N | 2 | 1 |

以前每行右边没有东西，多算一列看不出来；一旦画表格的右边框，`⏭ 已延后` 那两行就短一格。
而且**目标文字和 note 里可能出现任意 emoji**，手写区间表覆盖不了。
所以换成 `unicode-width` crate（0 依赖、编译期唯一新增），`style::width` 缩成一行。
验证方式：`status` 的每一行按 EAW 重算宽度，8 个 README 样例表 + 实机输出全部各行等宽。

## S4（中，指错人）`会话` 那行给的是旧会话

`session::summarize` 返回的行按**首次出现**排序（`order` 是 first-insert 时 push 的），
而取值处用 `sessions.last()`（status）和 `.rev().find()`（context）——都只是把"首次出现"倒过来。
一个会话先露过面、之后长期没动，照样会被挑中。

实机现象：干活的是 `7bc1af4c`（最后一条 tick 21:49），而 `status` 印的是 `01119e7e`（最后一条 tick 20:48）。
`claude --resume` 照着敲会进错会话。

修法：加 `session::latest(rows, host: Option<&str>)`，按 `last` 取最大；两个调用点都改过来。
回归测试 `the_session_line_points_at_whoever_worked_last`：A 先干、B 插一轮、A 再干，
手工把三条 tick 的时间戳拉开（秒级时间戳会撞），断言 status 和 context 都给 A。

## 这一类缺陷怎么避免

四条里有三条的形状一模一样：**同一个事实有两个来源，而两边的口径不一致**。
`is_terminal` vs 百分比分母、执行顺序 vs 创建顺序、"首次出现" vs "最近活动"。
它们都不会让程序崩，也不会被测试抓到（测试断言的是自己写下的那套口径），
只有把两个数放在同一屏里给人看的时候才露馅——所以**实机看一眼**这件事没法用测试替代。

`cargo test` 80 passed / 0 failed；改动涉及 `cli.rs` / `style.rs` / `session.rs` / `context.rs`
和 README 的 8 个样例块（用脚本按新排版重排，逐块校验各行等宽）。

---

# L3 的处置（t16）：锁超时要说清被谁挡住了

## 改了什么

| # | 修法 | 位置 |
|---|---|---|
| 1 | 拿到锁就在锁文件**旁边**写一条持有者记录：`{pid, op, at}`；释放前删掉（`HolderGuard` 的 `Drop`，panic 也算） | `state.rs write_holder/clear_holder`（`state.json.lock.holder`） |
| 2 | 超时那句话读这条记录：谁持着、持了多久、那个进程还在不在，再给处置步骤 | `state.rs timeout_error` |
| 3 | 操作名（`op`）由 CLI 按子命令设一次，runner 每轮细化成「run 第 N 轮」 | `cli.rs cmd_label` + `cli::run`、`runner.rs`（`state::set_operation`） |
| 4 | 5 秒这一档收成 `state::LOCK_WAIT` 一个常量，`state.rs` / `cli.rs` / `goals.rs` 共用 | `state::LOCK_WAIT` |

超时输出（实测，`/tmp` 上的一次性项目，外面用 `python3 fcntl.flock` 真持着锁）：

```
zloop: could not lock /private/tmp/ztest/.zloop/state.json.lock within 5.0s
持有者：pid 8928 · run 第 19 轮，拿到锁 7.4 秒了（进程还活着）
下一步：先看它在干什么 `ps -p 8928 -o command=`；确认真卡死了再 `kill 8928`；
        别删锁文件——内核锁才是权威，删了只会让两个进程同时写 state.json

# 没有持有者记录时（旧版 zloop 持的锁，或者进程被强杀）：
持有者：没有持有者记录（旧版 zloop 持的锁，或者进程被强杀没来得及留）
下一步：`lsof /private/tmp/ztest/.zloop/state.json.lock` 看谁开着它；别删锁文件
```

## 两个和 loopx 不一样的地方，都是故意的

**1）记录写在锁文件旁边，不写进锁文件本身。** loopx 的 `_holder_record` 是 `json.dump` 进锁文件
（`file_lock.py:157`）。锁文件的**内容**没有任何锁保护——就地覆写是"先截断再写"，等锁的那个人正好读在
中间，就只能读到半条 JSON，于是超时提示时好时坏。旁边那份走 tmp + rename，读者要么读到完整的旧记录、
要么读到完整的新记录。这跟 `state::save` 和 `daemon::write_pid` 是同一个理由（pid 文件那次就是被
`fs::write` 的截断窗口坑过：`status` 把活着的 runner 报成"没有 runner 在跑"）。

**2）清记录必须在**锁**里做。** `clear_holder` 放在 `drop(guard)` 之前：放到锁外面的话，
下一个人可能已经拿到锁并写了自己那份，我们这一手 `remove_file` 正好把他的记录删掉——
于是他持锁期间别人看到的是"没有持有者记录"。

**记录过期怎么办**：进程被 `kill -9`，内核会放锁，记录却留在盘上。所以超时时先 `kill(pid, 0)` 探一下，
死了就明说"这条是旧记录，真正持锁的是另一个进程"，并给 `lsof`——**不**替用户删文件，也不假装它有效。

## "只读命令用更短的等待"这条，实测的结论不一样

审计里写的是"`zloop status` / `context` / `log` 现在也要等满 5 秒"（L3 一节），
按这个说法应该给它们配 loopx 的 `MONITOR` 那一档（1 秒）。**实际读一遍代码：zloop 的只读命令根本不上锁。**
`status` / `context` / `log` / `doctor` / `stats` / `doc` 走的都是 `state::load`，
而 `save` 是 tmp + rename，读者只会看见换过去之前或之后的完整一份——所以它们的等待不是"更短"，是 0。

于是这一条的做法是**钉住**而不是新加一档：`tests/lock_test.rs` 里
`write_waits_and_reports_while_reads_go_straight_through` 在真持锁的情况下跑这三条命令，
断言各自退出 0 且耗时 < 2 秒（同一时刻 `zloop pause` 等满 5 秒并报出持有者）。
哪天有人往读路径上加锁，这条测试先红。加一个没人用的 `LOCK_WAIT_READ` 常量只会让人以为读路径要等锁。

## 没做的两件事

- **超时留档**（loopx 的 `.lock.incidents.jsonl`）：zloop 已经有 `runner/journal.jsonl` 和
  `.zloop/log/`，再开一个只在超时时才写的文件，值不回它的复杂度。真需要复盘时，超时那句话已经进了控制台日志。
- **`SINGLE_FLIGHT`（试一次就走）那一档**：现在没有调用方需要"拿不到就立刻放弃"。等真有了再加。

## 回归测试（`tests/lock_test.rs`，6 条）

| 用例 | 钉的是 |
|---|---|
| `timeout_names_the_live_holder` | 报错里有 pid、操作名、持有时长、"进程还活着"、处置步骤 |
| `holder_record_is_written_while_held_and_cleared_after` | 持锁期间记录在、内容对；放锁之后文件必须消失 |
| `holder_record_is_cleared_even_if_the_closure_panics` | 闭包 panic 展开也要清记录（`HolderGuard` 的 `Drop`）——否则留下一个 pid 还活着的假持有者 |
| `stale_holder_record_is_called_out_instead_of_believed` | pid 已经死了就明说记录是旧的，并给 `lsof`（pid 取自一个真的跑完退出的进程） |
| `missing_holder_record_still_says_what_to_do` | 没有记录时也得给下一步 |
| `write_waits_and_reports_while_reads_go_straight_through` | 端到端：真持锁时写命令等满 5 秒并报出持有者，只读三条命令 < 2 秒退出 0 |

`state::set_operation` 是进程级的，所以这几条用例用一把 `Mutex` 串起来跑——并行跑会互相改掉操作名。

`cargo test` 118 passed / 0 failed。

---

# F9 的处置（t17）：`goal rm` 猜出来的匹配要先问一句

## 问题回顾

`goal rm` 和 `goal switch` 共用一个 `resolve`，三档匹配：**id 精确 → id 前缀 → 目标文字包含**。
`zloop goal rm 冷启动` 只要文字片段命中唯一，就直接把 `.zloop/goals/<id>.json` 搬进
`.zloop/archive/`，**事后**才打印搬了谁。文件没丢，但这个动作没有 dry-run 也没有确认。

真正的不对称在这里：`switch` 猜错了，再 `switch` 回去就行；`rm` 猜错了，那个目标从
`goal list` 里消失，要去 `.zloop/archive/` 里按时间戳翻文件、手工搬回 `goals/` 才回得来。
同一个 `resolve`，两种风险等级。

## 改了什么

| # | 修法 | 位置 |
|---|---|---|
| 1 | `resolve` 现在会说自己是靠哪一档对上的：`Match::{Id, IdPrefix, Text}`，`is_fuzzy()` = 不是精确 id | `goals.rs resolve_match`（`resolve` 退化成它的 wrapper，`switch` 不变） |
| 2 | `archive` 改收 `&Row` 而不是 needle：对上了谁要先给用户看过，函数里再 resolve 一次就可能搬走另一个 | `goals.rs archive` |
| 3 | "当前目标不能归档"拆成 `ensure_archivable`，在**问之前**先拒 | `goals.rs ensure_archivable` |
| 4 | 猜出来的匹配：打印将要归档的（id、目标全文、状态、进度）+ 是按哪一档对上的 + 免问写法，然后等一句 `y` | `cli.rs cmd_goal` 的 `Rm` 分支 |
| 5 | `--yes` / `-y` 跳过确认 | `cli.rs GoalCmd::Rm` |

实测输出：

```
$ zloop goal rm 冷启动
将要归档 [g1] 把冷启动降到 1 秒 · 进行中 0/0
（"冷启动" 是按 目标文字片段 对上的，不是精确 id；免问：zloop goal rm g1 --yes）
确认归档？ [y/N] n
已取消，一个文件都没动          # 退出码 1

$ zloop goal rm keep-awake      # 精确 id：一句都不问，和以前一模一样
已归档 [keep-awake] 让 keep-awake 支持外接显示器 → …/.zloop/archive/…-keep-awake.json
```

## 三个决定，都不是默认写法

**1）id 前缀也要问，不只是文字片段。** issue 的验收只点了文字片段。但"精确 id"这一档的意义是
**用户准确说出了要动哪一个**，前缀不是——`goal rm g` 在只剩一个 `g` 开头的目标时会命中它，
用户想的可能是"`g` 是我随手打的一半"。分界线画在"说清楚了 / 我替他猜的"，比画在"哪一档"更好解释。
精确 id 的行为一个字节都没变，验收里"用精确 id 时保持现状不变"照旧。

**2）不看 stdin 是不是终端。** 直觉写法是 `if stdin().is_terminal() { 问 } else { 直接干 }`，
但那样等于：这条确认在所有脚本、所有 CI、所有测试里都不存在——最需要防手滑的批量场景反而全裸。
而且这条路会变成**测不到**的路（测试进程的 stdin 是管道，永远走 else）。
所以 `confirm` 无条件读一行 stdin，管道喂 `y\n` 和人手敲 `y` 走同一段代码。

**3）EOF 不当成"默认不同意"。** stdin 直接 EOF（`</dev/null`、runner 里跑的）说明根本没人接话。
悄悄退个非零码，调用方只会看到一个没解释的失败；所以这里 `bail` 并明说"这一步要确认，
但 stdin 没有输入可读：用精确 id 重来，或者加 `--yes`"。三种退出码分得开：
**0** 归档了 · **1** 用户说不 · **2** 报错（含没人接话、当前目标、匹配不到 / 匹配到多个）。

## 回归测试（`tests/cli_test.rs`）

`archiving_by_a_guessed_needle_asks_first_but_an_exact_id_does_not` 一条走完全部出口：

| 断言 | 钉的是 |
|---|---|
| 文字片段 + stdin EOF | 退 2，先打印"将要归档 …"和 `--yes` 写法，`goal list` 一个不少，**连 `.zloop/archive/` 目录都不建** |
| 答 `n` / 回车 / `别` | 三种都退 1、打印"已取消"、清单不变 |
| 当前目标 + 片段 + `y` | 退 2 报"是当前目标"，且**没有**打印过"确认归档"——不能问完 y 再说其实不能归档 |
| 文字片段 + `y` | 退 0，真搬走 |
| id 前缀（`g`）| 也要问，且输出里说明是按"id 前缀"对上的 |
| `--yes` + 片段 | 退 0 且不含"确认归档"（stdin 是空的，证明它根本没去读） |
| 精确 id | 退 0，输出里既没有"将要归档"也没有"确认归档" |
| 收尾 | `.zloop/archive/` 里正好 3 份 —— 三次都只是搬家，不是删除 |

`cargo test`：119 passed / 0 failed。
