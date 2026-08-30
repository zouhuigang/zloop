# 合盖不睡：zloop runner 运行期间阻止 Mac 休眠

> 需求：**有 zloop 任务在跑时，合上盖子任务不能停；没有任务时，合盖恢复 macOS 默认（息屏并休眠）。**
> 日期：2026-08-28。本机：macOS 26.5（Darwin 25）、Apple Silicon、电池供电。

## 1. macOS 上到底有哪几种"不睡"，各能防什么

| 机制 | 防什么 | 防不了什么 | 权限 | 怎么恢复 | 本机验证 |
|---|---|---|---|---|---|
| `caffeinate -i` | 空闲休眠（`PreventUserIdleSystemSleep` 断言） | 合盖；电池上的系统休眠 | 无 | 进程退出即失效（`-w <pid>` 跟随某进程） | ✓ `pmset -g assertions` 可见 |
| `caffeinate -s` | 接电源时的系统休眠（`PreventSystemSleep`） | **合盖**；电池供电时无效（"valid only when system is running on AC power"） | 无 | 同上 | ✓ |
| IOKit 断言 + `AppliesToLimitedPower`（公开 key） | 让上面的断言在电池上也生效 | **合盖** | 无 | 同上 | ✓ 可创建 |
| IOKit 断言 + `AppliesOnLidClose`（私有 key） | **合盖** | — | **需要 `com.apple.private.iokit.assertonlidclose` 授权，第三方拿不到**（macOS 10.13 起） | — | ✗ 本机 `IOPMAssertionCreateWithProperties` 返回 `0xe00002c1 = kIOReturnNotPrivileged` |
| **`pmset -a disablesleep 1`**（写 `SleepDisabled` 系统设置） | **合盖、电池、一切自动休眠** | 用户主动选"睡眠"菜单 | **root**（`sudo`） | 必须显式 `disablesleep 0`；**跨重启持久** | ✓ `pmset -g` 中存在 `SleepDisabled 0` 键；写入待 sudo 免密后实测 |
| 系统设置「接电源时显示器关闭后阻止自动休眠」 | 接电源 + 显示器关时不休眠 | 合盖（电池）；且是全局常驻设置 | 无 | 手动关 | 本机已开（`pmset -g custom` AC 段 `sleep 0`） |

结论：**合盖不睡只有一条路——`disablesleep`，需要 root。** 空闲休眠可以用无权限的 `caffeinate` 兜底。

## 2. 开源方案怎么做的

| 方案 | 做法 | 对 zloop 的启示 |
|---|---|---|
| **Fermata**（iccir/Fermata，"deactivate the lid close sensor"） | 1.0 用私有 `kIOPMAssertionAppliesOnLidClose` + `IOPMAssertionDeclareUserActivity()`，Apple 在 10.13 加授权后**改用 `pmset -b disablesleep 1`**；README 直言"unsupported by Apple, could break"，并警告"makes it easier to accidentally toss a running laptop into your bag… overheating or damage" | 私有 API 路线已死；`disablesleep` 是唯一可行解；**必须自动恢复**，不能靠人记得关 |
| **AwakeToggle**（MachineFriendly） | "The whole feature is one command: `sudo pmset -a disablesleep 1`"，每次切换弹系统管理员密码框；"sleep stays disabled until you explicitly turn it off, **including across reboots**" | 跨重启持久是最大坑：runner 若在机器重启前没来得及恢复，之后一直不睡 |
| **Amphetamine**（App Store） | 声称"publicly-accessible API"实现无外接显示器的 Closed-Display Mode；细节在登录墙后未能核实 | 不可复现，不依赖 |
| **KeepingYouAwake / LennardKittner/Caffeinate** | 都是 `caffeinate` 的菜单栏封装；后者 TODO 里写"Even if the lid is closed, prevent the Mac from sleeping. This probably requires root privileges" | 无权限方案的上限就是 `caffeinate` |
| **openclaw #15444**（长驻 gateway 防睡） | 讨论 `caffeinate -s`（"covers 95% of cases"，仅 AC）vs 原生 `PreventSystemSleep` 断言；未涉及合盖 | 服务类程序的主流选择只解决"插电不睡" |

## 3. zloop 的方案

两层叠加，**都跟随 runner 进程的生命周期**，runner 一死就恢复默认：

```
zloop start
  └─ runner (pid P)
       ├─ caffeinate -i -s -w P             ← 无权限层：空闲/接电源不睡；P 退出它自动退出
       ├─ sudo -n pmset -a disablesleep 1    ← 合盖层：需要一次性配置免密（zloop install --sudoers）
       │    └─ 登记 holder：~/.zloop/awake/<P>（多项目同时跑时引用计数）
       └─ watchdog: while kill -0 P; sleep; done; zloop awake reconcile   ← P 被 kill -9 也能恢复
runner 正常退出 → 注销 holder → 没有其他存活 holder 就 disablesleep 0
```

- **没有 sudo 免密**：只做 `caffeinate` 层，打印一次提示 `zloop install --sudoers`；不报错、不阻塞。
- **`zloop install --sudoers`**：写 `/etc/sudoers.d/zloop-pmset`，只放行三条精确命令：
  `pmset -a disablesleep 1` / `pmset -a disablesleep 0` / `pmset -g`。用 `visudo -c` 校验后再 `sudo install -m 0440`，会弹一次密码。
  规则先落在一个随机名的 **0700 私有目录**里（文件 0600），不落在临时目录本身：`sudo install` 会从那条路径**重新读一遍**，
  路径要是别人也占得住名字，装进 `/etc/sudoers.d/` 的就可能不是我们写的那份（[T43](FINDINGS.md#3-逐条草稿)，正文见 [CODE-AUDIT §20](CODE-AUDIT.md#t43中install_sudoers-的暂存路径别人也能占名装进-etcsudoersd-的可以不是我们写的那份--已修)）。
- **引用计数**：holder 文件按 pid 存在 `~/.zloop/awake/`；`reconcile` 清掉 pid 已死的 holder，只要还有存活的就保持 1，否则恢复 0。两个项目同时跑、一个先停，不会误关另一个。
- **`zloop status`** 增加一行 `sleep:`：`default` / `lid-close sleep disabled by zloop (N runner)` / `⚠ SleepDisabled=1 but no runner alive → zloop awake reconcile`。`zloop start` 与 `zloop stop` 都先 `reconcile` 一次，顺手清理上次没来得及恢复的状态。
- **`--no-keep-awake`**：不想动睡眠策略时关掉。
- **跨重启**：`disablesleep` 跨重启持久。若机器在 runner 运行中重启，开机后第一次 `zloop start/stop/status/awake` 会发现"1 但无 holder"并恢复。README 明写这一条，并建议装一个登录时跑 `zloop awake reconcile` 的 LaunchAgent（P2，可选）。
- **风险提示**：合盖运行意味着放进包里也在跑——发热、掉电。zloop 只在有 todo 可跑时保持不睡，`stopped`/`waiting` 一到就恢复；但"任务很长 + 电池"的组合仍需用户自己判断，建议插电。

## 4. 验证记录

### 4.1 自动化测试（`tests/runner_test.rs`，假 `sudo` / `pmset` / `caffeinate` 放在 PATH 前面，`HOME` 指向临时目录）

| 用例 | 场景 | 断言 |
|---|---|---|
| `runner_disables_lid_sleep_while_alive_and_restores_after` | 一个 runner 正常跑完 | `pmset` 调用序列恰为 `disablesleep 1` → `disablesleep 0`；journal 有 `awake_on{lid:true}` 与 `awake_off{restored_default:true}`；holder 目录清空；`--no-keep-awake` 时 pmset 一次都没被调 |
| `without_passwordless_sudo_runner_degrades_to_caffeinate_with_a_hint` | `sudo -n` 失败 | 不调 pmset；输出含 `run zloop install --sudoers once`；`awake_on{lid:false}`；`zloop awake` 显示 unavailable |
| `watchdog_restores_default_after_kill_9_and_holders_are_reference_counted` | 两个项目各 `zloop start`；停一个；再 `kill -9` 另一个 | 两个 holder 时 `SleepDisabled=1` 且 `awake` 显示 "2 runners"；停一个仍为 1；`kill -9` 后 watchdog（轮询 1s）在 10s 内恢复 0，holder 清空 |
| `awake_reconcile_fixes_a_stale_setting` | 模拟"上次没恢复"：`SleepDisabled=1` 但无 holder | `awake` 显示 ⚠；`awake reconcile` 输出 `restored to 0`；`status` 有 `sleep:` 行 |

### 4.2 什么时候恢复默认，什么时候不恢复

**开盖不是恢复条件。** 代码里没有任何地方监听盖子或唤醒事件（`grep -i 'lidopen|didWake|NSWorkspace|IONotification' src/` 为空），恢复只发生在这四处：

| 触发 | 恢复吗 | 说明 |
|---|---|---|
| 合上盖子 → 过一会儿打开 | **不恢复** | 什么都没发生；runner 继续跑，`SleepDisabled` 保持 1 |
| 任务自己跑完（`stopped (done)`），或因 `fail_streak` / `progress_streak` / `budget` 停机 | **自动恢复** | runner 退出 → `awake::release` → 无存活 holder → `disablesleep 0`。**不需要你敲任何命令** |
| 你主动 `zloop stop` | 恢复 | SIGTERM → runner 优雅退出 → 同上；`zloop stop` 本身也会 reconcile 一次 |
| runner 被 `kill -9` / panic / 机器重启 | 兜底恢复 | Drop guard 立即恢复；进程被强杀时由 watchdog（默认 15 s 轮询）恢复；重启这种极端情况由下一次 `zloop start/stop/status/awake` 的 reconcile 修正 |

自动化证明：`runner_test::sleep_stays_disabled_until_the_task_finishes_by_itself` —— 起一个跑 3 轮的 runner，全程无人干预，断言运行期间 `SleepDisabled` 持续为 1、runner 持续存活，任务自己跑完后 `SleepDisabled` 自动回到 0，且 `pmset` 全程只被调用了两次（一次开、一次关）。

### 4.3 真机验证步骤（macOS 26.5）

下面是**验证脚本**，不是日常用法：日常只有 `zloop start` 和"等它自己跑完"。sudo 层需要先执行一次 `zloop install --sudoers`（要输密码）。

```bash
zloop start                              # 任意项目，随便放几条 todo
pmset -g | grep SleepDisabled            # 期望 1
zloop status | grep sleep                # 期望 "lid-close sleep disabled by zloop (1 runner)"

# 合上盖子 1–2 分钟，再打开。打开后先看：任务应该还在推进，睡眠仍是禁用的
pmset -g log | grep 'Entering Sleep' | tail -3   # 期望：合盖期间没有新的 Entering Sleep
zloop status | grep -E 'phase|sleep'             # 期望：phase 有推进；sleep 仍显示 disabled by zloop
tail -5 .zloop/runner/console.log                # 期望：能看到合盖期间跑完的轮次

# 到这里验证已经完成。下面两行只是为了顺带确认"停了会恢复"——
# 日常不需要执行，任务自己跑完时会自动恢复。
zloop stop
pmset -g | grep SleepDisabled            # 期望 0
```

### 安装状态（2026-08-28 复核）

`zloop install --sudoers` 已执行：`/etc/sudoers.d/zloop-pmset` 存在（`-r--r----- root:wheel`），`sudo -n pmset -g` 免密可用，`zloop awake` 显示 `sleep: default (lid-close protection ready; a running runner will enable it)` —— 即合盖保护已就绪，下一个 `zloop start` 会自动启用。

### 真机记录（2026-08-28，macOS 26.5，未配置 sudoers）

```
$ zloop start
  runner: keep-awake: lid-close sleep is NOT disabled (needs passwordless `sudo pmset`); run `zloop install --sudoers` once to enable it. Idle sleep is held off by caffeinate.
$ zloop status | grep -E 'runner|sleep'
  runner: running in background (pid 30490) · log /tmp/awake-demo/.zloop/runner/console.log
  sleep: default · lid-close protection unavailable — run `zloop install --sudoers` once
$ pgrep -lf 'caffeinate -i -s -w 30490'
  30492 caffeinate -i -s -w 30490
$ pmset -g assertions | grep -A1 'pid 30492'
     pid 30492(caffeinate): [0x0003f906000195bb] 00:00:02 PreventUserIdleSystemSleep named: "caffeinate command-line tool"  
  	Details: caffeinate asserting on behalf of Process ID 30490
  --
     pid 30492(caffeinate): [0x0003f906000795bc] 00:00:02 PreventSystemSleep named: "caffeinate command-line tool"  
  	Details: caffeinate asserting on behalf of Process ID 30490
$ pmset -g | grep SleepDisabled
   SleepDisabled		0
$ zloop stop
  stopped runner (pid 30490)
$ pgrep -lf 'caffeinate -i -s -w 30490' || echo gone
  gone (caffeinate -w follows the runner pid)
$ tail -3 .zloop/runner/journal.jsonl
  {"event":"awake_on","lid":false,"caffeinate_pid":30492,"at":"2026-08-28T06:16:10+08:00"}
  {"event":"begin","round":1,"todo":"t1","host":"claude","resume":null,"at":"2026-08-28T06:16:10+08:00"}
```

结论：无 sudo 时 caffeinate 层正常挂在 runner pid 上、随 runner 退出释放；`SleepDisabled` 保持系统默认 0；`zloop stop`（SIGTERM）走优雅退出，journal 以 `stop(sigterm)` 收尾、无假 `restart`。sudo 层（合盖）待执行 `zloop install --sudoers` 后按 §4.2 步骤验证。



## 来源

- Fermata README：https://github.com/iccir/Fermata/blob/main/README.md
- IOPMLibPrivate.h（`kIOPMAssertionAppliesOnLidClose`）：https://github.com/opensource-apple/IOKitUser/blob/master/pwr_mgt.subproj/IOPMLibPrivate.h
- AwakeToggle 说明：https://www.machinefriendly.com/blog/keep-macbook-awake-lid-closed-awaketoggle
- lidrun：https://lidrun.com/blog/keep-mac-awake-when-lid-closed
- LennardKittner/Caffeinate：https://github.com/LennardKittner/Caffeinate
- openclaw #15444：https://github.com/openclaw/openclaw/issues/15444
