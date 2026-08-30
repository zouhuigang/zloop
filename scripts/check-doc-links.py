#!/usr/bin/env python3
"""文档链接闸：仓库里每一条跨文档 / 锚点链接都必须真的落得到地方。

    python3 scripts/check-doc-links.py          # 退 0 = 全过
    python3 scripts/check-doc-links.py --list    # 顺带把每个文件的锚点列出来（写文档时查名字用）

为什么要有这道闸：`docs/CODE-AUDIT.md` 三千多行、二十来节，索引（现在的
`docs/FINDINGS.md`）是靠「正文 §N」把人送回正文的。这种引用**没有编译器**——
写歪了、正文改了号，链接照样是一段普通文字，谁也不会报错。t45 立这道闸时
现场就抓到两类腐烂：

  1. 两节都叫 `## 6.`（第三轮和第四轮），11 处「正文 §6」有一半指错地方；
  2. 开头两处「见 §2 的 A-1 / B-1」，A-1 和 B-1 其实在 §4。

三条规则（任一不过就退 1，并把每条不过的都印出来）：

  R1 链接落得到：`[x](path#anchor)` / `[x](#anchor)` 里的文件要存在、锚点要对得上
     真实标题。锚点按 GitHub 的算法（github-slugger）现算——见 `slug()`。
  R2 编号不重不断：`docs/CODE-AUDIT.md` 和 `docs/FINDINGS.md` 里形如 `## N. 标题`
     的节号必须是 1..K 连续且不重复。
  R3 §N 有着落：`docs/CODE-AUDIT.md` 里每个 `§N` / `§N.M` 都要对得上本文件的一个标题；
     `docs/FINDINGS.md` 里每个 `§` 都必须**待在链接里**（那份是索引，光写个号等于没指路）。

锚点算法和 GitHub 对不对得上，是 `gh api /markdown/raw` 逐条比对过的（t45），
但那要联网，所以这里只留纯本地实现；改 `slug()` 之前请重新比对一次。
"""

import re
import sys
import unicodedata
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 只查仓库自己写的 Markdown（根目录 + docs/），不下 target/ 之类的坑。
DOCS = sorted(ROOT.glob("*.md")) + sorted((ROOT / "docs").glob("*.md"))

# R2 / R3 只对这两份「编号当索引用」的文档生效，别的文档爱怎么编号怎么编号。
NUMBERED = {"docs/CODE-AUDIT.md", "docs/FINDINGS.md"}
INDEX_DOC = "docs/FINDINGS.md"  # 这一份里的 § 必须是链接

LINK_RE = re.compile(r"\[(?P<text>(?:[^\[\]]|\[[^\]]*\])*)\]\((?P<href>[^()\s]*(?:\([^()]*\)[^()\s]*)*)\)")
HEADING_RE = re.compile(r"^(?P<hashes>#{1,6})\s+(?P<text>.*?)\s*$")
SECTION_RE = re.compile(r"^(?P<num>\d+)\.\s")
SECTION_REF_RE = re.compile(r"§(?P<num>\d+(?:\.\d+)*)")
FENCE_RE = re.compile(r"^\s*(```|~~~)")


def strip_inline(text: str) -> str:
    """把标题里的行内标记去掉，只留渲染后看得见的字。

    `[文字](链接)` 只留「文字」——不然链接里的字母会被算进锚点。反引号 / 星号
    这类本来就是标点，`slug()` 会顺手扔掉，这里不用管。
    """
    prev = None
    while prev != text:
        prev = text
        text = LINK_RE.sub(lambda m: m.group("text"), text)
    return text


def slug(text: str) -> str:
    """GitHub 的锚点算法：扔掉标点/符号（`-` `_` 除外）、空白变连字符、转小写。

    `①`（Unicode 类别 No）也是要扔的——`### 15.2 T40-①（中高）…` 在 GitHub 上
    的锚点是 `152-t40-中高…`，实测过。
    """
    out = []
    for ch in strip_inline(text).strip():
        if ch in "-_":
            out.append(ch)
        elif ch in " \t":
            out.append("-")
        else:
            cat = unicodedata.category(ch)
            if cat[0] in "PSC" or cat == "No":
                continue
            out.append(ch.lower())
    return "".join(out)


def headings(path: Path):
    """按出现顺序返回 (层级, 原文, 锚点)；同名标题按 GitHub 的规矩加 `-1` `-2`。

    围栏代码块里的 `#` 是注释不是标题，要跳过。
    """
    seen = {}
    result = []
    in_fence = False
    for line in path.read_text(encoding="utf-8").splitlines():
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        m = HEADING_RE.match(line)
        if not m:
            continue
        base = slug(m.group("text"))
        n = seen.get(base, 0)
        seen[base] = n + 1
        result.append((len(m.group("hashes")), m.group("text"), base if n == 0 else f"{base}-{n}"))
    return result


def links(path: Path):
    """返回 (行号, 链接文字, 目标)，跳过围栏代码块里的样例。"""
    result = []
    in_fence = False
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for m in LINK_RE.finditer(line):
            result.append((lineno, m.group("text"), m.group("href")))
    return result


def section_numbers(path: Path):
    """`## N. 标题` 里的 N，按出现顺序。"""
    return [int(SECTION_RE.match(text).group("num")) for lvl, text, _ in headings(path) if lvl == 2 and SECTION_RE.match(text)]


def heading_numbers(path: Path):
    """所有形如 `N.` / `N.M` 的标题编号，给 R3 当查表用。"""
    nums = set()
    for _lvl, text, _anchor in headings(path):
        m = re.match(r"^(\d+(?:\.\d+)*)[.\s]", text)
        if m:
            nums.add(m.group(1).rstrip("."))
    return nums


def rel(path: Path) -> str:
    return str(path.relative_to(ROOT))


def check_links(path: Path, anchors_of, problems):
    for lineno, text, href in links(path):
        if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", href) or href.startswith("//"):
            continue  # http(s):, mailto: 之类，本地查不了
        target, _, anchor = href.partition("#")
        if target:
            dest = (path.parent / target).resolve()
            if not dest.exists():
                problems.append(f"{rel(path)}:{lineno}: 链接指向不存在的路径 `{href}`（[{text}]）")
                continue
        else:
            dest = path
        if not anchor:
            continue
        if dest.suffix != ".md":
            continue  # 非 Markdown 的 #fragment 不归这道闸管
        anchors = anchors_of(dest)
        if anchors is None:
            continue  # 仓库外的 Markdown，不查
        if anchor not in anchors:
            near = [a for a in anchors if anchor[:6] and a.startswith(anchor[:6])]
            hint = f"；最接近的：{near[0]}" if near else ""
            problems.append(f"{rel(path)}:{lineno}: 锚点对不上 `#{anchor}`（在 {rel(dest)} 里没有这个标题{hint}）")


def check_numbering(path: Path, problems):
    nums = section_numbers(path)
    dupes = sorted({n for n in nums if nums.count(n) > 1})
    if dupes:
        problems.append(f"{rel(path)}: 有重复的节号 §{'、§'.join(str(d) for d in dupes)}——「正文 §N」就指不准了")
    if nums and sorted(set(nums)) != list(range(1, max(nums) + 1)):
        missing = sorted(set(range(1, max(nums) + 1)) - set(nums))
        problems.append(f"{rel(path)}: 节号不连续，缺 §{'、§'.join(str(m) for m in missing)}")


def check_section_refs(path: Path, own_numbers, problems):
    """CODE-AUDIT：每个 §N 都要指得到本文件的一个标题。"""
    in_fence = False
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        for m in SECTION_REF_RE.finditer(line):
            if m.group("num") not in own_numbers:
                problems.append(f"{rel(path)}:{lineno}: `§{m.group('num')}` 在本文件里没有对应的标题")


def check_index_refs_are_links(path: Path, problems):
    """FINDINGS：每个 § 都得待在链接里——索引光写个号不算指路。"""
    in_fence = False
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        spans = [m.span() for m in LINK_RE.finditer(line)]
        for m in SECTION_REF_RE.finditer(line):
            if not any(a <= m.start() and m.end() <= b for a, b in spans):
                problems.append(f"{rel(path)}:{lineno}: `§{m.group('num')}` 是裸的——索引里的节号要写成锚点链接")


def main() -> int:
    cache = {}

    def anchors_of(dest: Path):
        key = dest.resolve()
        if key not in cache:
            try:
                key.relative_to(ROOT)
            except ValueError:
                return None
            cache[key] = {a for _lvl, _text, a in headings(key)}
        return cache[key]

    if "--list" in sys.argv:
        for path in DOCS:
            print(f"\n=== {rel(path)}")
            for lvl, text, anchor in headings(path):
                print(f"{'  ' * (lvl - 1)}#{anchor}\t{text}")
        return 0

    problems = []
    for path in DOCS:
        check_links(path, anchors_of, problems)
        if rel(path) in NUMBERED:
            check_numbering(path, problems)
    audit = ROOT / "docs" / "CODE-AUDIT.md"
    if audit.exists():
        check_section_refs(audit, heading_numbers(audit), problems)
    index = ROOT / INDEX_DOC
    if index.exists():
        check_index_refs_are_links(index, problems)

    if problems:
        for p in problems:
            print(f"[doc-links] {p}", file=sys.stderr)
        print(f"\n[doc-links] {len(problems)} 处没过。", file=sys.stderr)
        return 1
    n = sum(len(links(p)) for p in DOCS)
    print(f"[doc-links] 全过：{len(DOCS)} 个文件、{n} 条链接。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
