# 每条 todo 留一份技术文档

> `zloop done` 的 --approach / --decision / --pitfall / --evidence 写进 `.zloop/log/`，别人接手时读得懂。
>
> ← 回到 [README](../../README.md)

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
