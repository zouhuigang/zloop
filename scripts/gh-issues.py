#!/usr/bin/env python3
"""GitHub issue ↔ zloop todo 的薄绑定。

**为什么是脚本不是 zloop 子命令**：zloop 的承诺是"一个 JSON 文件、十几条命令、不依赖外部服务"。
把 GitHub 拉进去意味着它要管认证、网络错误、API 变更——那不是调度器该操心的事。
todo 的文本本来就是自由的，把 `#12` 写进去就够当链接用了。

约定：todo 文本里带 `(#12)` 就算绑定到 issue 12。

  pull   open issues → todo 行（`--apply` 直接写进 zloop）
  close  已完成且带 issue 号的 todo → 评论 + 关闭（默认只预览，`--yes` 才真动手）
  status 对一遍：哪些 todo 绑了 issue、各自什么状态
"""
import argparse, json, re, subprocess, sys

ISSUE_RE = re.compile(r"#(\d+)")

def gh(*args, check=True):
    r = subprocess.run(["gh", *args], capture_output=True, text=True)
    if check and r.returncode != 0:
        print(f"gh {' '.join(args)} 失败：{r.stderr.strip()}", file=sys.stderr)
        sys.exit(1)
    return r.stdout

def zloop(*args):
    r = subprocess.run(["zloop", *args], capture_output=True, text=True)
    return r.stdout, r.returncode

def state():
    out, code = zloop("status", "--json")
    if code != 0:
        print("读不到 zloop 状态（当前目录有 .zloop 吗？）", file=sys.stderr)
        sys.exit(2)
    return json.loads(out)

def issue_no(text):
    m = ISSUE_RE.search(text)
    return int(m.group(1)) if m else None

def cmd_pull(a):
    raw = gh("issue", "list", "--repo", a.repo, "--state", "open", "--limit", str(a.limit),
             "--json", "number,title,labels,body")
    issues = json.loads(raw or "[]")
    if a.label:
        issues = [i for i in issues if any(l["name"] == a.label for l in i.get("labels", []))]
    if not issues:
        print("没有符合条件的 open issue")
        return 0
    # 已经绑过的不重复拉
    bound = {issue_no(t["text"]) for t in state()["todos"]}
    lines = []
    for i in issues:
        if i["number"] in bound:
            continue
        # issue body 里的「验收：」那一行直接当 acceptance
        acc = ""
        for l in (i.get("body") or "").splitlines():
            if l.strip().startswith(("验收：", "验收:", "Acceptance:")):
                acc = " :: " + l.split("：", 1)[-1].split(":", 1)[-1].strip()
                break
        lines.append(f"[P{a.priority}] {i['title']} (#{i['number']}){acc}")
    if not lines:
        print("open issue 都已经在计划里了")
        return 0
    for l in lines:
        print(l)
    if a.apply:
        args = []
        for l in lines:
            args += ["--add", l]
        out, code = zloop("plan", *args)
        print(out.strip())
        return code
    print("\n（加 --apply 才真的写进 zloop）")
    return 0

def cmd_close(a):
    st = state()
    todos = [t for t in st["todos"] if issue_no(t["text"])]
    acted = 0
    for t in todos:
        n = issue_no(t["text"])
        # **只关已经 done 的**：progress / fail / blocked / deferred 一律不碰
        if t["status"] != "done":
            print(f"  跳过 #{n}（{t['id']} 是 {t['status']}，没完成）")
            continue
        info = json.loads(gh("issue", "view", str(n), "--repo", a.repo, "--json", "state,title"))
        if info["state"] != "OPEN":
            print(f"  跳过 #{n}（issue 已经是 {info['state']}）")
            continue
        tick = next((k for k in reversed(st["ticks"]) if k.get("todo") == t["id"] and k["outcome"] == "done"), None)
        body = [f"由 zloop 完成：**{t['text']}**", ""]
        if tick:
            body.append(f"- 结果：{tick.get('note') or '（无）'}")
            if tick.get("log"):
                body.append(f"- 过程记录：`.zloop/{tick['log']}`")
            body.append(f"- 完成于：{tick['at']}")
        if t.get("acceptance"):
            body.append(f"- 验收标准：{t['acceptance']}")
        msg = "\n".join(body)
        if not a.yes:
            print(f"  [预览] 会关闭 #{n} 并评论：\n    " + msg.replace("\n", "\n    "))
            continue
        gh("issue", "comment", str(n), "--repo", a.repo, "--body", msg)
        gh("issue", "close", str(n), "--repo", a.repo)
        print(f"  ✅ 关闭 #{n}（{t['id']}）")
        acted += 1
    if not a.yes:
        print("\n（加 --yes 才真的评论并关闭）")
    return 0

def cmd_status(a):
    st = state()
    rows = [(t["id"], issue_no(t["text"]), t["status"], t["text"]) for t in st["todos"]]
    if not rows:
        print("没有 todo")
        return 0
    for tid, n, s, text in rows:
        tag = f"#{n}" if n else "—"
        print(f"  {tid:<4} {tag:<6} {s:<9} {text[:60]}")
    return 0

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default="zouhuigang/zloop")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pull"); p.add_argument("--label"); p.add_argument("--limit", type=int, default=20)
    p.add_argument("--priority", type=int, default=1); p.add_argument("--apply", action="store_true")
    p.set_defaults(fn=cmd_pull)
    p = sub.add_parser("close"); p.add_argument("--yes", action="store_true"); p.set_defaults(fn=cmd_close)
    p = sub.add_parser("status"); p.set_defaults(fn=cmd_status)
    a = ap.parse_args()
    return a.fn(a)

if __name__ == "__main__":
    sys.exit(main())
