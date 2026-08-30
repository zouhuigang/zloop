#!/usr/bin/env python3
"""文档链接闸：仓库里每一条跨文档 / 锚点链接、每一个 `§N` 都必须真的落得到地方。

    python3 scripts/check-doc-links.py            # 退 0 = 全过
    python3 scripts/check-doc-links.py --list     # 顺带把每个文件的锚点列出来（写文档时查名字用）
    python3 scripts/check-doc-links.py --self-test  # 拿合成文档验这几条规则自己还灵不灵

为什么要有这道闸：`docs/audit/CODE-AUDIT.md` 三千多行、二十来节，索引（现在的
`docs/audit/FINDINGS.md`）是靠「正文 §N」把人送回正文的。这种引用**没有编译器**——
写歪了、正文改了号，链接照样是一段普通文字，谁也不会报错。t45 立这道闸时
现场就抓到两类腐烂：

  1. 两节都叫 `## 6.`（第三轮和第四轮），11 处「正文 §6」有一半指错地方；
  2. 开头两处「见 §2 的 A-1 / B-1」，A-1 和 B-1 其实在 §4。

t46 把 R2 / R3 从「只管 CODE-AUDIT 和 FINDINGS」推广到 **docs/ 下全部 15 份 + README**，
当场又抓到一类躲过前两类的腐烂：`docs/design/DESIGN.md` 写「notes §4.3」指的是
`loopx-scheduling-notes.md` 的 §4.3，可 DESIGN 自己也有一个 §4.3（`tick.outcome`）——
号**指得到**，只是指到了另一份文档的同号节上。所以 R3 现在要先判「这个 §N 说的是**哪份**
文档」，再判「那份文档里有没有这一号」。

t47 把「归属靠猜」这件事本身判成了缺陷：三级归属能告诉你这个号**大概**说的是哪份文档，
但归属对不对没人验、落点更无从谈起。跨文档引用写成锚点链接就两样都被验（R1 验落点、
R3a 验号和落点是同一节），所以 R5 直接把裸的跨文档引用判红——三级归属从此只是兜底，
留给写不成链接的那一种（目标文档不在这个仓库里）。

五条规则（任一不过就退 1，并把每条不过的都印出来）：

  R1 链接落得到：`[x](path#anchor)` / `[x](#anchor)` 里的文件要存在、锚点要对得上
     真实标题。锚点按 GitHub 的算法（github-slugger）现算——见 `slug()`。
  R2 编号不重不断：形如 `## N. 标题` 的节号必须不重复、不跳号，且第一节只能是 §0 或 §1
     （`§0` 开头是允许的——ADAPTIVE-REPLAN / LONG-RUN-PROOF 等都从 §0 起；但「从 §2 起」
     是删了节没重编号，指向 §1 的引用从此无处可落）。
  R3 §N 有着落：每个 `§N` / `§N.M` 都要对得上**某份文档**的一个标题。归属按这个顺序判：
       a. § 待在一条链接的文字里 → 说的是那条链接指向的文档；这时还额外要求**号和落点
          一致**（写 §20 就得落在 §20 名下的标题上，不能号写对锚点却落在别节）；
       b. 否则看同一行、§ 之前最近提到的 `xxx.md` → 说的是那份文档；
       c. 都没有 → 说的是本文件。
     归属到仓库外的文档（loopx 上游的 `field-derived-patterns.md` 之类）查不了，跳过并计数。
     豁免：`§N` 待在**行内代码**里（`` `§7.1` ``）＝在引用这个写法本身，不查——讨论规则的
     文字和遵守规则的文字长得一模一样，反引号是唯一分得开的地方。同理，行内代码里的
     `[链接](x.md#y)` 也不当真链接查（R1 一样豁免）。
     已知取舍：b 只看**同一行**、且认的是写全了的 `xxx.md`。不跨行是故意的，跨行猜归属的
     假阳性比它挡住的腐烂还多。b 归属到**本仓库另一份文档**的那一种现在归 R5 管（判红），
     所以 b 剩下的活只有两件：认出仓库外的目标（跳过），和认出「提到的就是自己」（当自指查）。
  R4 索引里的 § 必须待在链接里：`docs/audit/FINDINGS.md` 那份是索引，光写个号等于没指路。
  R5 跨文档的 § 必须待在链接里（写作约定，全仓生效）：`§N` 指的是本仓库**另一份**文档时，
     不许裸着写。裸写的两处代价——归属靠「同一行最近提到的 `xxx.md`」猜（猜错了不会有人报错），
     落点根本没验（号对得上就算过，落在哪一小节无从谈起）。写成
     `[xxx.md §N](xxx.md#锚点)` 之后 R1 验落点存在、R3a 验号和落点是同一节。
     豁免两种：目标文档不在这个仓库里（写不成链接，只能裸着引，跳过并计数），
     以及同一行提到的就是本文件（那是自指，不是跨文档）。

锚点算法和 GitHub 对不对得上，是 `gh api /markdown/raw` 逐条比对过的（t45），
但那要联网，所以这里只留纯本地实现；改 `slug()` 之前请重新比对一次。

规则自己也会退化，所以 `--self-test` 拿一组合成文档把每条规则该报的都点一遍：
真实文档全绿时，「规则还灵不灵」就只有它能回答了。
"""

import re
import shutil
import sys
import tempfile
import unicodedata
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

INDEX_DOC = "docs/audit/FINDINGS.md"  # 这一份里的 § 必须是链接

LINK_RE = re.compile(r"\[(?P<text>(?:[^\[\]]|\[[^\]]*\])*)\]\((?P<href>[^()\s]*(?:\([^()]*\)[^()\s]*)*)\)")
HEADING_RE = re.compile(r"^(?P<hashes>#{1,6})\s+(?P<text>.*?)\s*$")
SECTION_RE = re.compile(r"^(?P<num>\d+)\.\s")
SECTION_REF_RE = re.compile(r"§(?P<num>\d+(?:\.\d+)*)")
HEADING_NUM_RE = re.compile(r"^(?P<num>\d+(?:\.\d+)*)[.\s]")
# 行内提到的文档名（可带路径）：`loopx-scheduling-notes.md`、docs/design/DESIGN.md、concepts/field-derived-patterns.md
DOC_MENTION_RE = re.compile(r"(?P<name>[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\.md)")
FENCE_RE = re.compile(r"^\s*(```|~~~)")
# 行内代码：`` `§7.1` `` 是在**引用这个写法本身**，不是在指路。讨论规则的文字和遵守规则的
# 文字长得一模一样，这是唯一分得开的地方——包进反引号即豁免（全仓实测只有 2 处，都是引用写法）。
INLINE_CODE_RE = re.compile(r"``[^`]+``|`[^`]*`")
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def collect_docs(root: Path):
    """只查仓库自己写的 Markdown（根目录 + docs/ 全部层级），不下 target/ 之类的坑。

    用 rglob 而不是 glob：docs/ 分成了 guide/ design/ audit/ 三层之后，只扫一层
    会让这道闸静默缩水成「只查 README」——踩过：分完文件夹当场从 16 个文件掉到 1 个，
    而它照样打印「全过」。**闸的覆盖面缩小是不会报错的**，所以下面还要断言文件数。
    """
    docs = sorted(root.glob("*.md")) + sorted((root / "docs").rglob("*.md"))
    return [d for d in docs if "target" not in d.parts]


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


def body_lines(path: Path):
    """按行号产出正文（跳过围栏代码块——里面的样例不该被当成真链接/真引用）。"""
    in_fence = False
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE_RE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        yield lineno, line


def quoted(line: str):
    """这一行里被行内代码包住的区间——落在里面的链接和 §N 都是「引用写法」，不查。"""
    return [m.span() for m in INLINE_CODE_RE.finditer(line)]


def inside(span, spans) -> bool:
    return any(a <= span[0] and span[1] <= b for a, b in spans)


def links(path: Path):
    """返回 (行号, 链接文字, 目标)，跳过围栏代码块和行内代码里的样例。"""
    out = []
    for lineno, line in body_lines(path):
        code = quoted(line)
        out += [(lineno, m.group("text"), m.group("href")) for m in LINK_RE.finditer(line) if not inside(m.span(), code)]
    return out


def section_numbers(path: Path):
    """`## N. 标题` 里的 N，按出现顺序。"""
    return [int(SECTION_RE.match(text).group("num")) for lvl, text, _ in headings(path) if lvl == 2 and SECTION_RE.match(text)]


def heading_numbers(path: Path):
    """所有形如 `N.` / `N.M` 的标题编号，给 R3 当查表用。"""
    return {m.group("num").rstrip(".") for _lvl, text, _anchor in headings(path) if (m := HEADING_NUM_RE.match(text))}


def anchor_owners(path: Path):
    """锚点 → 它待在哪个顶层节（最近的一个 `## N.` 的号）；节前的锚点归 None。

    R3a 用它把「链接文字里写的号」和「锚点真正的落点」对起来：`[正文 §20](CODE-AUDIT.md#x)`
    里 `#x` 必须是 §20 底下的标题，不能号写着 20、人却落到 §19 去。
    """
    owners = {}
    cur = None
    for lvl, text, anchor in headings(path):
        m = HEADING_NUM_RE.match(text)
        if lvl == 2 and m:
            cur = m.group("num").rstrip(".")
        owners[anchor] = cur
    return owners


def rel(path: Path, root: Path) -> str:
    return str(path.relative_to(root))


def in_repo(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def resolve_doc(name: str, base: Path, root: Path):
    """把行内提到的文档名解析成仓库里的路径；解析不到（仓库外的文档）返回 None。

    先按「相对写它的那份文档」找——README 里的 `docs/design/DESIGN.md` 和 docs/ 里的
    `DESIGN.md` 指的是同一份；再兜底试 `docs/` 和仓库根。
    """
    for cand in (base.parent / name, root / "docs" / name, root / name):
        cand = cand.resolve()
        if cand.suffix == ".md" and cand.exists() and in_repo(cand, root):
            return cand
    return None


def check_links(path: Path, root: Path, anchors_of, problems):
    """R1：每条本地链接的文件要存在、锚点要对得上真实标题。"""
    for lineno, text, href in links(path):
        if SCHEME_RE.match(href) or href.startswith("//"):
            continue  # http(s):, mailto: 之类，本地查不了
        target, _, anchor = href.partition("#")
        if target:
            dest = (path.parent / target).resolve()
            if not dest.exists():
                problems.append(f"{rel(path, root)}:{lineno}: 链接指向不存在的路径 `{href}`（[{text}]）")
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
            problems.append(f"{rel(path, root)}:{lineno}: 锚点对不上 `#{anchor}`（在 {rel(dest, root)} 里没有这个标题{hint}）")


def check_numbering(path: Path, root: Path, problems):
    """R2：`## N.` 的节号不重复、不跳号，且第一节的号只能是 §0 或 §1。

    下界不能写死 1：多份文档是从 `## 0.`（前情 / 结论先行）起编的。但也不能一路取
    `min(nums)` 了事——那样「§1 被删了、后面没重编」就成了合法的，而这正是
    「§N 指不到」的上游成因之一。所以分成两句话：起点只准是 0 或 1，起点之后不准跳号。
    """
    nums = section_numbers(path)
    if not nums:
        return
    dupes = sorted({n for n in nums if nums.count(n) > 1})
    if dupes:
        problems.append(f"{rel(path, root)}: 有重复的节号 §{'、§'.join(str(d) for d in dupes)}——「正文 §N」就指不准了")
    lo = min(nums)
    if lo > 1:
        problems.append(f"{rel(path, root)}: 节号从 §{lo} 起——第一节只能是 §0 或 §1，多半是删了节没重编号")
    missing = sorted(set(range(lo, max(nums) + 1)) - set(nums))
    if missing:
        problems.append(f"{rel(path, root)}: 节号不连续，缺 §{'、§'.join(str(m) for m in missing)}")


def check_section_refs(path: Path, root: Path, numbers_of, owners_of, anchors_of, problems, stats):
    """R3：每个 §N 都要指得到某份文档的一个标题（先判归属哪份，再判有没有这一号）。"""
    for lineno, line in body_lines(path):
        code = quoted(line)
        spans = [(m.span(), m.group("href")) for m in LINK_RE.finditer(line) if not inside(m.span(), code)]
        mentions = [(m.start(), m.group("name")) for m in DOC_MENTION_RE.finditer(line)]
        for m in SECTION_REF_RE.finditer(line):
            if inside(m.span(), code):
                continue  # `§N` 是在引用写法本身
            num = m.group("num")
            stats["refs"] += 1
            href = next((h for (a, b), h in spans if a <= m.start() and m.end() <= b), None)

            if href is not None:  # a. 待在链接里 → 说的是链接指向的那份
                if SCHEME_RE.match(href) or href.startswith("//"):
                    stats["outside"] += 1
                    continue  # 指向站外的链接，本地查不了
                target, _, anchor = href.partition("#")
                dest = resolve_doc(target, path, root) if target else path
                if dest is None:
                    stats["outside"] += 1
                    continue
                where = "在本文件里" if dest.resolve() == path.resolve() else f"在 {rel(dest, root)} 里"
                # 号和落点得是同一节：R1 只保证锚点存在，保证不了它待在 §num 底下。
                if anchor and anchor in (anchors_of(dest) or ()):
                    owner = owners_of(dest).get(anchor)
                    if owner != num.split(".")[0]:
                        landed = f"§{owner}" if owner else "第一节之前"
                        problems.append(f"{rel(path, root)}:{lineno}: 链接文字写 `§{num}`，锚点 `#{anchor}` 却落在 {rel(dest, root)} 的 {landed}")
                        continue
            else:  # b. 同一行、§ 之前最近提到的 xxx.md；c. 都没有 → 本文件
                name = next((n for start, n in reversed(mentions) if start < m.start()), None)
                dest, where = path, "在本文件里"
                if name is not None:
                    target_doc = resolve_doc(name, path, root)
                    if target_doc is None:
                        stats["outside"] += 1
                        continue  # 仓库外的文档（loopx 上游那几份）：写不成链接，也查不了
                    if target_doc.resolve() != path.resolve():
                        # R5：裸的跨文档引用。归属是猜的、落点没验——写成链接两样都被验。
                        problems.append(
                            f"{rel(path, root)}:{lineno}: `§{num}` 指的是 {rel(target_doc, root)} 的节，却是裸的"
                            f"——跨文档引用要写成锚点链接 `[{name} §{num}]({name}#锚点)`"
                        )
                        continue
                    # 提到的就是本文件：那是自指，不是跨文档，按 c 查。

            if num not in numbers_of(dest):
                problems.append(f"{rel(path, root)}:{lineno}: `§{num}` {where}没有对应的标题")


def check_index_refs_are_links(path: Path, root: Path, problems):
    """R4：FINDINGS 里每个 § 都得待在链接里——索引光写个号不算指路。"""
    for lineno, line in body_lines(path):
        code = quoted(line)
        spans = [m.span() for m in LINK_RE.finditer(line)]
        for m in SECTION_REF_RE.finditer(line):
            if inside(m.span(), code):
                continue  # `§N` 是在引用写法本身
            if not any(a <= m.start() and m.end() <= b for a, b in spans):
                problems.append(f"{rel(path, root)}:{lineno}: `§{m.group('num')}` 是裸的——索引里的节号要写成锚点链接")


def run_checks(root: Path):
    """把四条规则跑一遍，返回 (问题列表, 统计)。root 可以是仓库，也可以是 --self-test 的合成目录。

    `root` 先 `resolve()`：路径归属（`in_repo`）比的是解析过的路径，而 macOS 的
    `tempfile.mkdtemp()` 给的是 `/var/folders/…`（`/private/var/…` 的符号链接）——
    不解析就会判定合成文档「在仓库外」，于是 R1/R3 全体静默跳过，self-test 只剩空转。
    """
    root = Path(root).resolve()
    docs = collect_docs(root)
    anchors_cache, numbers_cache, owners_cache = {}, {}, {}

    def anchors_of(dest: Path):
        key = dest.resolve()
        if key not in anchors_cache:
            if not in_repo(key, root):
                return None
            anchors_cache[key] = {a for _lvl, _text, a in headings(key)}
        return anchors_cache[key]

    def numbers_of(dest: Path):
        key = dest.resolve()
        if key not in numbers_cache:
            numbers_cache[key] = heading_numbers(key)
        return numbers_cache[key]

    def owners_of(dest: Path):
        key = dest.resolve()
        if key not in owners_cache:
            owners_cache[key] = anchor_owners(key)
        return owners_cache[key]

    problems = []
    stats = {"docs": len(docs), "links": 0, "refs": 0, "outside": 0}
    for path in docs:
        stats["links"] += len(links(path))
        check_links(path, root, anchors_of, problems)
        check_numbering(path, root, problems)
        check_section_refs(path, root, numbers_of, owners_of, anchors_of, problems, stats)
    index = root / INDEX_DOC
    if index.exists():
        check_index_refs_are_links(index, root, problems)
    return problems, stats


# --------------------------------------------------------------------------- self-test

# 每份合成文档配一句「该报什么」：真实文档全绿的时候，只有它能回答「规则还灵不灵」。
#
# 这些路径是**自包含的假仓库**，故意保持扁平，不跟着 docs/ 的真实分层走：
# 它测的是规则本身，跟真仓库怎么摆目录无关。踩过——docs/ 分成 guide/design/audit
# 三层时批量改路径把这里也改了，合成的 BROKEN.md 与 CODE-AUDIT.md 就跨了目录，
# 自检当场失灵。
FIXTURES = {
    "docs/CODE-AUDIT.md": """# 正文

## 0. 前情
从 §0 起编号是允许的，这一份不该因为「缺 §」被报。

## 1. 第一节
见 §2 的说明，也见 [§2 的说明](#2-第二节)。

### 1.1 子节
指得到的自指：§1.1。

## 2. 第二节
好的跨文档引用（写成链接）：[`loopx-scheduling-notes.md` §1.9](loopx-scheduling-notes.md#19-子节)。
仓库外的文档写不成链接，应当跳过：`field-derived-patterns.md` §7。
引用写法本身，不查：`§99`，以及 `[样例](NOPE.md#不存在)`。
""",
    "docs/loopx-scheduling-notes.md": """# 笔记

## 0. 前情

## 1. 第一节

### 1.9 子节
本文件的 §1.9 存在，而引用它的 CODE-AUDIT 里没有这一号——跨文档归属判错就会现形。
""",
    "docs/DESIGN.md": """# 设计

## 1. 第一节

### 1.3 子节
自指指不到：§8.8。
同号异档：[`loopx-scheduling-notes.md` §1.3](loopx-scheduling-notes.md)——链接没带锚点时号照样要按**链接指向的那份**查，本文件有同号的一节，那边没有。
提到的是自己，不算跨文档：`DESIGN.md` §1.3 就在本文件里，R5 不该报。
""",
    "docs/BROKEN.md": """# 坏样例

## 1. 甲
## 1. 乙
## 4. 丙

坏路径：[没有这个文件](NOPE.md)。
坏锚点：[没有这个标题](CODE-AUDIT.md#不存在的标题)。
号与落点不符：[正文 §2](CODE-AUDIT.md#1-第一节)。
裸的跨文档引用：`CODE-AUDIT.md` §1——号明明存在，可归属是猜的、落点一点没验。
""",
    "docs/GAP.md": """# 起点被删了

## 2. 乙
第一节被删掉、后面没重编号，于是号从 §2 起——原先指向它的引用从此无处可落。

## 3. 丙
""",
    "docs/audit/FINDINGS.md": """# 索引

## 1. 一览
裸号：正文 §1。
""",
}

# 期待的报告**逐字全集**：不是「至少包含」，是「一条不多一条不少」。
# 少一条 = 规则失灵；多一条 = 规则误伤（假阳性同样会让人把闸关掉）。
EXPECTED = [
    ("R1 坏路径", "docs/BROKEN.md:7: 链接指向不存在的路径 `NOPE.md`（[没有这个文件]）"),
    ("R1 坏锚点", "docs/BROKEN.md:8: 锚点对不上 `#不存在的标题`（在 docs/CODE-AUDIT.md 里没有这个标题）"),
    ("R2 重号", "docs/BROKEN.md: 有重复的节号 §1——「正文 §N」就指不准了"),
    ("R2 断号", "docs/BROKEN.md: 节号不连续，缺 §2、§3"),
    ("R2 起点不是 §0/§1", "docs/GAP.md: 节号从 §2 起——第一节只能是 §0 或 §1，多半是删了节没重编号"),
    ("R3 自指指不到", "docs/DESIGN.md:6: `§8.8` 在本文件里没有对应的标题"),
    ("R3 同号异档", "docs/DESIGN.md:7: `§1.3` 在 docs/loopx-scheduling-notes.md 里没有对应的标题"),
    ("R3a 号与落点不符", "docs/BROKEN.md:9: 链接文字写 `§2`，锚点 `#1-第一节` 却落在 docs/CODE-AUDIT.md 的 §1"),
    ("R4 索引裸号", "docs/audit/FINDINGS.md:4: `§1` 是裸的——索引里的节号要写成锚点链接"),
    (
        "R5 裸的跨文档引用",
        "docs/BROKEN.md:10: `§1` 指的是 docs/CODE-AUDIT.md 的节，却是裸的"
        "——跨文档引用要写成锚点链接 `[CODE-AUDIT.md §1](CODE-AUDIT.md#锚点)`",
    ),
]

# 合成文档里还埋了这几处**该沉默**的，靠「一条不多」兜住：`§0` 开头的编号（CODE-AUDIT / notes）、
# 指得到的自指（`§1.1`）、链接里的自指（`[§2 的说明](#2-第二节)`）、写成链接的跨文档引用（`§1.9`）、
# 仓库外文档的 §（`field-derived-patterns.md §7`，写不成链接所以 R5 豁免）、同一行提到的就是
# 本文件的那种（DESIGN 里的 `` `DESIGN.md` §1.3 ``，是自指不是跨文档）、以及行内代码里的 `§99`
# 和那条指向不存在文件的样例链接（引用写法本身，不查）。

def self_test() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="doc-links-self-test-"))
    try:
        for name, text in FIXTURES.items():
            p = tmp / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(text, encoding="utf-8")
        problems, _stats = run_checks(tmp)
        got, want = Counter(problems), Counter(msg for _label, msg in EXPECTED)
        bad = []
        for label, msg in EXPECTED:
            if got[msg] < want[msg]:
                bad.append(f"该报没报：{label} —— 期待 `{msg}`")
        for msg, n in (got - want).items():
            bad.append(f"多报了 {n} 条（规则误伤）：`{msg}`")
        if bad:
            print("[self-test] 规则失灵：", file=sys.stderr)
            for b in bad:
                print(f"  {b}", file=sys.stderr)
            print("\n[self-test] 合成文档上实际报出来的：", file=sys.stderr)
            for p in problems:
                print(f"  {p}", file=sys.stderr)
            return 1
        print(f"[self-test] 全过：合成文档上报出来的正好是期待的那 {len(EXPECTED)} 条，一条不多一条不少。")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()

    if "--list" in sys.argv:
        for path in collect_docs(ROOT):
            print(f"\n=== {rel(path, ROOT)}")
            for lvl, text, anchor in headings(path):
                print(f"{'  ' * (lvl - 1)}#{anchor}\t{text}")
        return 0

    problems, stats = run_checks(ROOT)
    if problems:
        for p in problems:
            print(f"[doc-links] {p}", file=sys.stderr)
        print(f"\n[doc-links] {len(problems)} 处没过。", file=sys.stderr)
        return 1
    outside = f"，另有 {stats['outside']} 处 § 指向仓库外的文档（查不了）" if stats["outside"] else ""
    print(f"[doc-links] 全过：{stats['docs']} 个文件、{stats['links']} 条链接、{stats['refs'] - stats['outside']} 处 §N{outside}。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
