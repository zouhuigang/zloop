//! 自动闸（`.github/workflows/ci.yml` + `scripts/check.sh`）的回归测试。
//! 四道：docs（文档链接与节号）→ fmt → clippy → test。
//!
//! 这里钉的不是「闸能不能跑通」——那是 CI 自己每次 push 都在回答的问题；这里钉的是
//! **闸只有一份定义**：CI 必须去调 `scripts/check.sh`，不许在 workflow 里再抄一遍
//! `cargo fmt --all` / `clippy -D warnings` / `cargo test` / 文档链接闸。抄成两份之后它们会各走各的，
//! 到那天「本地过了」和「CI 过了」就不是同一句话了，而这种漂移平时看不出来。
//!
//! 另外钉住 `runs-on: macos-*`：`awake::supported()` 是 `cfg!(target_os = "macos")`，
//! 非 macOS 上 keep-awake 整层是 no-op，7 个断言 pmset 真被调过的测试会开局就红
//! （实测：把 `supported()` 改成 `false` 跑 `cargo test` → runner_test 7 failed / 37 passed）。
//! 一道开局就红的闸等于没有闸——t30 已经在 `cargo fmt` 上踩过一次了。

use std::fs;
use std::path::PathBuf;

fn repo(rel: &str) -> String {
    let p: PathBuf = [env!("CARGO_MANIFEST_DIR"), rel].iter().collect();
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("读不到 {}：{e}", p.display()))
}

/// 去掉整行注释再看。注释里**本来就该**写着「不许内联 `-D warnings`」这类话，
/// 拿注释去判「有没有内联」会把讲解当成违规（第一版就这么自己红了一次）。
fn without_comments(s: &str) -> String {
    s.lines().filter(|l| !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n")
}

#[test]
fn ci_calls_the_same_gate_humans_call() {
    let ci = without_comments(&repo(".github/workflows/ci.yml"));
    assert!(ci.contains("scripts/check.sh"), "CI 必须调 scripts/check.sh：\n{ci}");

    // 四条实命令一条都不许出现在 workflow 的实际步骤里——出现即说明有人把闸抄成了第二份。
    // （`cargo fmt --version` / `cargo clippy --version` 是打印版本，不是闸，所以这里
    //   匹配的是带闸参数的形态。）
    for inlined in ["check-doc-links.py", "cargo fmt --all", "--all-targets", "-D warnings", "cargo test"] {
        assert!(!ci.contains(inlined), "workflow 里不许内联 `{inlined}`，闸的定义在 scripts/check.sh：\n{ci}");
    }
}

#[test]
fn the_gate_covers_docs_fmt_clippy_and_test() {
    let sh = repo("scripts/check.sh");
    for cmd in [
        "python3 scripts/check-doc-links.py",
        "cargo fmt --all -- --check",
        "cargo clippy --all-targets --all-features -- -D warnings",
        "cargo test",
    ] {
        assert!(sh.contains(cmd), "scripts/check.sh 少了这一道：`{cmd}`\n{sh}");
    }
    // 默认（不带参数）必须四道全跑，别哪天被改成只跑 fmt
    assert!(sh.contains(r#"${*:-"docs fmt clippy test"}"#), "check.sh 的默认闸必须是 docs fmt clippy test：\n{sh}");
}

fn doc_link_gate(args: &[&str]) -> std::process::Output {
    std::process::Command::new("python3")
        .arg("scripts/check-doc-links.py")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("跑不了 python3 scripts/check-doc-links.py")
}

/// 文档闸自己得是绿的——一道开局就红的闸等于没有闸（t30 在 `cargo fmt` 上踩过一次）。
/// 这里顺带把它当成回归测试用：`docs/CODE-AUDIT.md` 的节号重复过一次（第三轮和第四轮
/// 都编成 6），害得十一处「正文 §N」有一半指错地方，而当时没有任何东西会报错。
#[test]
fn the_doc_link_gate_is_green() {
    let out = doc_link_gate(&[]);
    assert!(
        out.status.success(),
        "文档链接闸红了：\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **规则自己也会退化**，而真实文档全绿的时候没有任何东西在回答「它还灵不灵」——
/// 上面那个测试红不了，恰恰因为闸什么都不查时它也是绿的。`--self-test` 拿一组合成
/// 文档把每条规则该报的点一遍（重号、断号、坏路径、坏锚点、自指指不到、同号异档、
/// 号与落点不符、索引裸号、裸的跨文档引用），外加几条**不该**报的（`§0` 开头、
/// 仓库外的文档、写成链接的跨文档引用、指得到的自指，以及「同一行提到的就是本文件」
/// ——那是自指，R5 不该把它误伤成跨文档）。
///
/// t46 把 R2/R3 从两份文档推广到全仓时，就是它当场抓住了自己写歪的两处：一是节号下界
/// 写死 1，于是所有从 `## 0.` 起编的文档都要报一句「节号不连续，缺 §」——缺的还是个
/// 空列表；二是 macOS 的 `mkdtemp()` 给的 `/var/folders/…` 是符号链接，路径没 `resolve()`
/// 就把合成文档判在「仓库外」，R1/R3 全体静默跳过、self-test 只剩空转。
#[test]
fn the_doc_link_gate_rules_still_bite() {
    let out = doc_link_gate(&["--self-test"]);
    assert!(
        out.status.success(),
        "文档闸的规则失灵了（合成文档上该报的没报，或不该报的报了）：\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn ci_runs_on_macos_because_seven_tests_need_it() {
    let ci = without_comments(&repo(".github/workflows/ci.yml"));
    let runs_on = ci.lines().find(|l| l.trim_start().starts_with("runs-on:")).expect("workflow 里没有 runs-on");
    assert!(runs_on.contains("macos"), "CI 得跑在 macOS 上，否则 keep-awake 的 7 个测试开局就红：{runs_on}");
}
