#!/usr/bin/env python3
"""长程运行自检器：判断某次 zloop 运行到底算不算「长程」。

**为什么需要它**：zloop 的账本是它自己写的，"跑了 9 轮"这种话由它自己说不算数。
这里只认那些**要真的发生过一次无头长跑才会存在**的痕迹，并且尽量取自 zloop 之外：

  1. runner journal 的 begin/end 序列  —— 只有 runner 每轮才写，交互轮次不写
  2. 墙钟跨度                          —— 第一个 begin 到最后一个 end

**窗口取最近一次运行，不是整个 journal**：journal 是追加的，同一个项目里 runner
起停过很多次。拿整份来量，会把上一次运行之前的人工 tick 和提交算进这一次的窗口，
于是「无人干预」这条必然挂掉——踩过：一次真跑了 4 小时的运行被判成 ❌，
挂掉的是长跑**开始之前**的 6 条人工 tick。要看累计口径用 --all。
  3. 窗口内没有人工 tick               —— 有人插手就不是"无人值守"
  4. 会话 resume 链                    —— 第 2 轮起接着上一轮的会话，证明跨轮连续
  5. 宿主报的 cost / duration          —— 真的调过 claude -p 才有
  6. 窗口内有 git 提交                 —— zloop 之外的证据，它伪造不了

只读：不写任何文件，不改任何状态。

用法：scripts/longrun-audit.py [--rounds N] [--hours H] [--dir PATH]
"""
import argparse, json, os, subprocess, sys
from datetime import datetime

def iso(s):
    try:
        return datetime.fromisoformat(s)
    except Exception:
        return None

def load_journal(root):
    p = os.path.join(root, ".zloop/runner/journal.jsonl")
    if not os.path.exists(p):
        return []
    out = []
    for line in open(p, encoding="utf-8"):
        line = line.strip()
        if line:
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return out

def runs(journal):
    """把 journal 切成一次次运行：`awake_on` / `restart` 是开机标记，各起一段。

    只保留跑过整轮（有 begin 也有 end）的段——没有 begin 的段是空跑，
    比如 `zloop start` 起来发现没活可做立刻 `stop`，那种不该被当成一次运行。
    """
    marks = [i for i, e in enumerate(journal) if e.get("event") in ("awake_on", "restart")]
    if not marks:
        return [journal] if journal else []
    if marks[0] != 0:
        marks.insert(0, 0)
    segs = [journal[a:b] for a, b in zip(marks, marks[1:] + [len(journal)])]
    real = [s for s in segs
            if any(e.get("event") == "begin" for e in s) and any(e.get("event") == "end" for e in s)]
    return real or segs


def load_states(root):
    """当前目标 + 停放的 + 归档的，全都算——长跑可能发生在任何一个目标上。"""
    import glob
    files = [os.path.join(root, ".zloop/state.json")]
    files += glob.glob(os.path.join(root, ".zloop/goals/*.json"))
    files += glob.glob(os.path.join(root, ".zloop/archive/*.json"))
    out = []
    for f in files:
        try:
            st = json.load(open(f, encoding="utf-8"))
            if isinstance(st, dict) and "ticks" in st:
                out.append(st)
        except Exception:
            pass
    return out

def git_commits_between(root, a, b):
    try:
        r = subprocess.run(
            ["git", "-C", root, "log", "--since", a.isoformat(), "--until", b.isoformat(), "--pretty=%h %ad %s", "--date=iso"],
            capture_output=True, text=True, timeout=15)
        return [l for l in r.stdout.strip().splitlines() if l]
    except Exception:
        return []

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dir", default=".")
    ap.add_argument("--rounds", type=int, default=6, help="至少几轮由 runner 驱动")
    ap.add_argument("--hours", type=float, default=2.0, help="至少多少小时墙钟")
    ap.add_argument("--all", action="store_true",
                    help="量整个 journal 的累计口径（默认只量最近一次运行）")
    args = ap.parse_args()
    root = os.path.abspath(args.dir)

    whole = load_journal(root)
    sessions = runs(whole)
    if args.all or not sessions:
        journal, which = whole, ""
    else:
        journal = sessions[-1]
        which = f"（第 {len(sessions)} 次运行，共 {len(sessions)} 次；--all 看累计）" if len(sessions) > 1 else ""
    begins = [e for e in journal if e.get("event") == "begin"]
    ends = [e for e in journal if e.get("event") == "end"]
    checks = []

    # 1) 轮次：只数 runner 写的 begin
    checks.append(("runner 驱动的轮次", len(begins) >= args.rounds,
                   f"{len(begins)} 轮（要求 ≥ {args.rounds}）"))

    # 2) 墙钟跨度
    span_h, t0, t1 = 0.0, None, None
    # 只认 begin/end 的时刻：段尾的 sleep/awake_off/stop 都在最后一轮**之后**，
    # 把它们算进跨度等于给自己送时间
    stamps = [iso(e["at"]) for e in begins + ends if e.get("at") and iso(e["at"])]
    if stamps:
        t0, t1 = min(stamps), max(stamps)
        span_h = (t1 - t0).total_seconds() / 3600
    checks.append(("墙钟跨度", span_h >= args.hours,
                   f"{span_h:.2f} 小时（要求 ≥ {args.hours}）" if stamps else "没有 journal"))

    # 3) 窗口内没有人工 tick：人工的痕迹是 edit / feedback，以及 via=next 留下的 in_progress
    human = 0
    if t0 and t1:
        for st in load_states(root):
            for tk in st.get("ticks", []):
                at = iso(tk.get("at", ""))
                if at and t0 <= at <= t1 and tk.get("outcome") in ("edit", "feedback"):
                    human += 1
    checks.append(("窗口内无人工干预", human == 0, f"{human} 条人工 tick（edit/feedback）"))

    # 4) 会话 resume 链：第 2 轮起带上了上一轮的会话
    resumed = sum(1 for b in begins if b.get("resume"))
    checks.append(("跨轮会话连续", resumed >= 1 and len(begins) >= 2,
                   f"{resumed}/{max(len(begins)-1,0)} 轮接续了上一轮的会话"))

    # 5) 宿主真的被调用过（cost 或 duration 由 claude -p 回报）
    reported = 0
    if t0 and t1:
        for st in load_states(root):
            for tk in st.get("ticks", []):
                at = iso(tk.get("at", ""))
                if at and t0 <= at <= t1 and (tk.get("cost_usd") or tk.get("duration_ms")):
                    reported += 1
    checks.append(("宿主回报了花费/耗时", reported >= 1, f"{reported} 轮带 cost/duration"))

    # 6) zloop 之外的证据：窗口内的 git 提交
    commits = git_commits_between(root, t0, t1) if t0 and t1 else []
    checks.append(("窗口内产出 git 提交", len(commits) >= 1, f"{len(commits)} 个提交"))

    # 列宽按显示宽度算，不按字符数——中文占两列（zloop 自己也踩过这个坑）
    def dw(t):
        import unicodedata
        return sum(2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1 for ch in t)
    w = max(dw(c[0]) for c in checks)
    print(f"\n  长程自检 · {root}")
    if t0 and t1:
        print(f"  窗口：{t0:%Y-%m-%d %H:%M} → {t1:%Y-%m-%d %H:%M} {which}\n")
    else:
        print("  窗口：没有 runner journal —— 这个项目里 runner 从没跑过\n")
    for name, ok, detail in checks:
        print(f"  {'✅' if ok else '❌'} {name}{' ' * (w - dw(name))}  {detail}")
    verdict = all(ok for _, ok, _ in checks)
    print(f"\n  结论：{'这是一次长程运行' if verdict else '**不是**长程运行'}\n")
    if commits:
        print("  窗口内的提交：")
        for c in commits[:10]:
            print(f"    {c}")
        print()
    return 0 if verdict else 1

if __name__ == "__main__":
    sys.exit(main())
