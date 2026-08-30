# 一个项目多个目标

> 当前目标躺在 `state.json`，其余停在 `.zloop/goals/`；切换就是换车，什么都不丢。
>
> ← 回到 [README](../../README.md)

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

### 边界：goals 只看得见当前项目（这是取舍，不是缺陷）

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

全局那一层今天只有一个 `~/.zloop/awake/`（keep-awake 的持有者计数，见 [docs/KEEP-AWAKE.md](../../docs/design/KEEP-AWAKE.md)），
**没有任何项目或目标的索引**（[DESIGN.md](../../docs/design/DESIGN.md) 的 G1）。为什么这么定：

- **全局 registry 的成本全在一致性上**：loopx 为此写了 842 行（`global_registry.py`）——同步、merge、
  冲突路由、退休清理，外加 4 个 global skill。zloop 换目标就是一次 rename，没有第二份真源，也就没有对不齐的问题。
- **索引一定会烂**：项目会改名、搬走、删掉，索引里的那行不会自己消失。烂掉的索引最难受的地方是，
  你只会在**真的想用它**的那一刻才发现它在列不存在的项目。
- **这个视图人一般不缺**：你知道自己手上有哪几个项目；真要一眼扫完，下面这行 shell 就够，不值得为它引入一份全局状态。

**今天已经有的手动路径**（实测输出就是上面那两段）：

```bash
for d in ~/work/*/; do [ -d "$d/.zloop" ] && (cd "$d" && echo "== $(basename "$d")" && zloop goals); done
```

### 如果要做：最小形态（没实现，[#8](https://github.com/zouhuigang/zloop/issues/8)）

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
