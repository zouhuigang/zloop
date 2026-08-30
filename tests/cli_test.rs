mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use zloop::hosts;
use zloop::state;

struct Out {
    code: i32,
    out: String,
    err: String,
}

fn zloop(dir: &Path, args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zloop"));
    cmd.current_dir(dir).args(args);
    common::scrub_ambient_env(&mut cmd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn zloop");
    if let Some(s) = stdin {
        child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
    }
    let o = child.wait_with_output().unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

#[test]
fn end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let o = zloop(d, &["init", "Ship zloop v0"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("initialized"));

    let o = zloop(d, &["init", "other"], None, &[]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("already initialized"));

    let o = zloop(d, &["plan"], Some("[P0] design\n[P1] build\n[P2] docs\n"), &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(o.out.lines().collect::<Vec<_>>(), ["t1 [P0] design", "t2 [P1] build", "t3 [P2] docs"]);

    let o = zloop(d, &["next", "--json"], None, &[]);
    let payload: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(payload["should_run"], true);
    assert_eq!(payload["todo"]["id"], "t1");
    assert_eq!(payload["remaining"], 3);
    assert_eq!(payload["round"], 0);
    assert_eq!(payload["interval_min"], 3);

    let o = zloop(
        d,
        &["done", "t1", "--note", "DESIGN.md written", "--next", "review design", "--evidence", "line1\nline2", "--no-doc"],
        None,
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.starts_with("t1 done: DESIGN.md written"));
    assert!(o.out.contains("next: t4 [P0] review design"));
    assert!(o.out.contains("log: .zloop/log/"));

    let o = zloop(
        d,
        &["done", "t4", "--outcome", "fail", "--note", "reviewer away", "--pitfall", "评审人休假，先跳过"],
        None,
        &[],
    );
    assert_eq!(o.code, 0);
    assert!(o.out.contains("t4 fail"));

    let o = zloop(d, &["done", "t4", "--block", "need product sign-off"], None, &[]);
    assert_eq!(o.code, 0);
    assert!(o.out.contains("t4 block"));
    // blocked P0 does not stop P1 from running
    let o = zloop(d, &["next", "--json"], None, &[]);
    let payload: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(payload["todo"]["id"], "t2");

    let o = zloop(d, &["edit", "t4", "--status", "open", "--priority", "2"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("t4 [P2] open"));

    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("Ship zloop v0"), "{}", o.out);
    assert!(o.out.contains("1 轮"), "{}", o.out);

    let o = zloop(d, &["status", "--md"], None, &[]);
    assert!(o.out.starts_with("# zloop"));
    assert!(o.out.contains("`t2`"));

    let st = state::load(&state::state_path(d)).unwrap();
    let outcomes: Vec<&str> = st.ticks.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(outcomes, ["done", "fail", "block", "edit"]);
    assert_eq!(st.ticks[0].host.as_deref(), Some("cli"));
    assert!(st.ticks[0].log.as_deref().unwrap().starts_with("log/"));

    // logs
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains("-t1-done.md"));
    let st = state::load(&state::state_path(d)).unwrap();
    let (files, _) = zloop::log::entries(d, &st, Some("t1"), 10).unwrap();
    assert_eq!(files.len(), 1);
    let body = fs::read_to_string(&files[0].0).unwrap();
    assert!(body.contains("## 验证证据") && body.contains("line2"));
    let o = zloop(d, &["log", "--show", files[0].0.file_name().unwrap().to_str().unwrap()], None, &[]);
    assert!(o.out.contains("- note: DESIGN.md written"));
}

#[test]
fn next_records_noop_unless_peek() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["done", "t1", "--block", "?"], None, &[]);
    zloop(d, &["next", "--peek"], None, &[]);
    zloop(d, &["next"], None, &[]);
    let o = zloop(d, &["next"], None, &[]);
    assert!(o.out.contains("WAIT (user_gate)"));
    let st = state::load(&state::state_path(d)).unwrap();
    let outcomes: Vec<&str> = st.ticks.iter().map(|t| t.outcome.as_str()).collect();
    assert_eq!(outcomes, ["block", "noop", "noop"]);
}

#[test]
fn done_errors() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["done", "t9", "--no-doc"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("unknown todo id"));
    zloop(d, &["done", "t1", "--no-doc"], None, &[]);
    let o = zloop(d, &["done", "t1", "--no-doc"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("already done"));
}

#[test]
fn missing_state_is_a_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let o = zloop(dir.path(), &["next"], None, &[]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("no zloop state"));
}

#[test]
fn heartbeat_hosts_and_budget() {
    let dir = tempfile::tempdir().unwrap();
    zloop(dir.path(), &["init", "a goal"], None, &[]);
    for host in ["claude", "codex-app", "codex-cli"] {
        let o = zloop(dir.path(), &["heartbeat", "--host", host], None, &[]);
        assert_eq!(o.code, 0);
        assert!(o.out.contains("zloop next --json"));
        assert!(o.out.chars().count() <= 1300, "{host}: {}", o.out.chars().count());
    }
}

#[test]
fn session_is_captured_from_host_env() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    let o = zloop(
        d,
        &["done", "t1", "--note", "x", "--no-doc"],
        None,
        &[("CLAUDE_CODE_SESSION_ID", "11111111-2222-3333-4444-555555555555")],
    );
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["done", "t2", "--outcome", "progress", "--note", "y"], None, &[("CODEX_THREAD_ID", "thread-abc")]);
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["sessions", "--json"], None, &[]);
    let rows: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["host"], "claude");
    assert_eq!(rows[0]["resume"], "claude --resume 11111111-2222-3333-4444-555555555555");
    assert_eq!(rows[1]["host"], "codex");
    assert_eq!(rows[1]["resume"], "codex resume thread-abc");
    let o = zloop(d, &["sessions", "--host", "codex"], None, &[]);
    assert!(o.out.contains("codex resume thread-abc"));
    assert!(!o.out.contains("claude --resume"));
    let o = zloop(d, &["status", "--md"], None, &[]);
    assert!(o.out.contains("## Sessions"));
}

#[test]
fn context_respects_budget_and_names_next() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "Long goal text for the context packet"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] first thing", "--add", "[P1] second thing"], None, &[]);
    zloop(d, &["done", "t1", "--note", "done first", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-1")]);
    let o = zloop(d, &["context", "--for", "codex"], None, &[]);
    assert!(o.out.contains("## 下一条") && o.out.contains("t2 [P1] second thing"));
    assert!(o.out.contains("claude --resume sess-1"));
    assert!(o.out.contains("在 Codex 里"));
    let o = zloop(d, &["context", "--budget", "300"], None, &[]);
    assert!(o.out.chars().count() <= 301, "{}", o.out.chars().count());
    assert!(o.out.contains("## 目标"));
}

/// 预算小到连保护区都放不下时的行为（#13）。
/// 三条不变量：不崩、不留半个章节、丢的顺序永远是"保护区最后走"。
#[test]
fn context_survives_absurdly_small_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "Long goal text for the context packet"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] first thing", "--add", "[P1] second thing"], None, &[]);
    zloop(d, &["remember", "done 之前一定要跑 cargo test", "--rule"], None, &[]);
    zloop(d, &["remember", "一条留给下一轮的经验"], None, &[]);
    zloop(d, &["done", "t1", "--note", "done first", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-1")]);

    let full = zloop(d, &["context"], None, &[]).out;
    // 保护区（到「下一条」为止）和它后面可裁的那几节，都得先在默认预算下真的出现过，
    // 不然下面的"谁先被丢"根本没在测东西
    for head in ["## 目标", "## 本项目的约定", "## 当前判断", "## 下一条"] {
        assert!(full.contains(head), "默认预算下应有 {head}：{full}");
    }
    for head in ["## 待办", "## 会话", "## 经验", "## 怎么继续"] {
        assert!(full.contains(head), "默认预算下应有 {head}：{full}");
    }

    // 0 也是合法预算：不能 panic，也不能吐出任何字符
    for budget in [0usize, 1, 3, 5, 7, 8, 20, 60, 120, 200, 300, 500, 700, 1000, 4000] {
        let o = zloop(d, &["context", "--budget", &budget.to_string()], None, &[]);
        assert_eq!(o.code, 0, "budget={budget} 不该失败：{}{}", o.out, o.err);
        let text = o.out.trim_end_matches('\n');
        let len = text.chars().count();
        assert!(len <= budget, "budget={budget} 却输出了 {len} 个字符：{text:?}");

        // 不产生半个章节：留下的每一行要么是完整小标题，要么是某个小标题下面的内容
        for line in text.lines() {
            assert!(!line.starts_with("##") || line.starts_with("## "), "budget={budget} 出现了被切断的标题 {line:?}");
        }
        // 光有标题没内容的尾巴也算半个章节
        assert!(!text.trim_end().ends_with("## 目标"), "budget={budget} 只剩一个光标题：{text:?}");
        // 老实现会把最后一段截在半个字上（"…"），现在只整节整行地丢
        assert!(!text.contains('…'), "budget={budget} 仍在半路截断：{text:?}");

        // 保护区优先：外层的节只要还在，说明里层的节一定也还在
        let has = |h: &str| text.contains(h);
        if has("## 怎么继续") {
            assert!(has("## 下一条"), "budget={budget}「怎么继续」活着而「下一条」被丢了：{text:?}");
        }
        if has("## 待办") || has("## 会话") || has("## 经验") {
            assert!(has("## 下一条") && has("## 怎么继续"), "budget={budget} 先丢了保护区/收尾：{text:?}");
        }
        if has("## 下一条") {
            assert!(
                has("## 当前判断") && has("## 本项目的约定") && has("## 目标"),
                "budget={budget} 保护区被穿了：{text:?}"
            );
        }
        if has("## 当前判断") {
            assert!(has("## 本项目的约定") && has("## 目标"), "budget={budget} 保护区被穿了：{text:?}");
        }
    }

    // 预算大到装得下全部时，一节都不该少
    let o = zloop(d, &["context", "--budget", "100000"], None, &[]);
    assert_eq!(o.out, full, "预算足够时输出应与默认一致");
}

#[test]
fn install_is_idempotent_and_refuses_unmanaged() {
    let home = tempfile::tempdir().unwrap();
    let results = hosts::install(true, true, true, home.path(), false).unwrap();
    assert!(results.iter().all(|w| w.changed));
    let again = hosts::install(true, true, true, home.path(), false).unwrap();
    assert!(again.iter().all(|w| !w.changed), "什么都没改的重装不该重写文件: {again:?}");
    let skill = home.path().join(".claude/skills/zloop/SKILL.md");
    let text = fs::read_to_string(&skill).unwrap();
    assert!(text.starts_with("---\nname: \"zloop\""));
    assert!(text.contains(hosts::MANAGED_PREFIX));
    assert!(text.contains("zloop context"));
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.path().join(".claude/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["hooks"]["Stop"][0]["hooks"][0]["command"], hosts::HOOK_COMMAND);
    fs::write(&skill, "# my own file\n").unwrap();
    assert!(hosts::install_claude(home.path(), false).is_err());
}

/// A-1：`~/.claude/settings.json` 是合法 JSON、但不是我要的形状时，别 panic 也别覆写。
///
/// 这个文件不属于 zloop——用户和别的工具都在写它，`"hooks": []` 这种写法完全可能出现。
/// 以前三层形状是三个 `.expect()`：`zloop install --claude-stop-hook` 直接 panic + exit 101
/// （`hosts.rs:262/267/270`）。对比之下"文件不是合法 JSON"那条路径处理得很好（报错 + 说明），
/// 这个反差本身就是结论：**校验了形状的有无，没校验形状对不对。**
///
/// 两条都要钉：报的错说得出是哪一层，以及**磁盘上那个文件一个字节都没动**。
#[test]
fn a_wrongly_shaped_settings_json_is_reported_not_panicked_or_clobbered() {
    // (文件内容, 错误里必须点名的那一层)
    let cases = [
        ("[]", "顶层"),
        ("\"hello\"", "顶层"),
        ("42", "顶层"),
        ("{\"hooks\": []}", "hooks"),
        ("{\"hooks\": \"none\"}", "hooks"),
        ("{\"hooks\": {\"Stop\": {}}}", "hooks.Stop"),
        ("{\"hooks\": {\"Stop\": \"off\"}}", "hooks.Stop"),
    ];
    for (raw, layer) in cases {
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join(".claude/settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, raw).unwrap();
        // 撤掉修复（换回 .expect）时这一行就是 panic，测试进程直接死
        let err = hosts::install_claude_stop_hook(home.path()).unwrap_err().to_string();
        assert!(err.contains(layer), "错误得说清是哪一层不对（{raw} → 期望点名 {layer}）：{err}");
        assert!(err.contains("没动这个文件"), "得明说没碰用户的全局配置：{err}");
        assert_eq!(fs::read_to_string(&settings).unwrap(), raw, "{raw}：用户的全局配置必须原样留着");
    }
    // 形状对的照旧写得进去，别把闸修成一堵墙
    for raw in ["{}", "{\"hooks\":{}}", "{\"hooks\":{\"Stop\":[]}}"] {
        let home = tempfile::tempdir().unwrap();
        let settings = home.path().join(".claude/settings.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(&settings, raw).unwrap();
        hosts::install_claude_stop_hook(home.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], hosts::HOOK_COMMAND, "{raw}");
    }
}

/// skill 是给人改的（Warp 那边它就是改进的载体）。所以：用户区永远保留，
/// 托管区被手改过就停下报错——绝不静默覆盖。
#[test]
fn install_keeps_your_edits_and_refuses_to_clobber_the_managed_part() {
    let home = tempfile::tempdir().unwrap();
    let skill = home.path().join(".claude/skills/zloop/SKILL.md");
    hosts::install_claude(home.path(), false).unwrap();
    assert!(fs::read_to_string(&skill).unwrap().contains(hosts::USER_MARK), "模板自带用户区，告诉人往哪写");

    // 1) 用户区里的内容跨重装保留，而且文件一个字节都不用动
    let mine = "- done 之前一定要跑 cargo test\n- 不要碰 migrations/\n";
    fs::write(&skill, format!("{}{mine}", fs::read_to_string(&skill).unwrap())).unwrap();
    let w = hosts::install_claude(home.path(), false).unwrap();
    assert!(!w[0].changed && w[0].kept_user > 0, "{w:?}");
    let text = fs::read_to_string(&skill).unwrap();
    assert!(text.contains("cargo test") && text.contains("migrations"), "{text}");

    // 2) 动了托管区 → 拒绝，并指出两条出路
    fs::write(&skill, text.replace("zloop context", "zloop ctx")).unwrap();
    let err = hosts::install_claude(home.path(), false).unwrap_err().to_string();
    assert!(err.contains("托管区被改过") && err.contains(hosts::USER_MARK) && err.contains("--force"), "{err}");
    assert!(fs::read_to_string(&skill).unwrap().contains("zloop ctx"), "被拒的安装不能动文件");

    // 3) --force 覆盖托管区，用户区照样留着
    hosts::install_claude(home.path(), true).unwrap();
    let text = fs::read_to_string(&skill).unwrap();
    assert!(text.contains("zloop context") && !text.contains("zloop ctx"), "托管区回到模板");
    assert!(text.contains("cargo test"), "用户区不受 --force 影响");

    // 4) 老版本装的文件（裸标记、没有指纹）：这次照旧覆盖，但把保护加上
    fs::write(&skill, "---\nname: \"zloop\"\n---\n\n<!-- zloop-managed:v1 -->\n# 旧版\n").unwrap();
    let w = hosts::install_claude(home.path(), false).unwrap();
    assert!(w[0].changed && w[0].migrated, "{w:?}");
    assert!(fs::read_to_string(&skill).unwrap().contains("fp="), "从此带上指纹");
}

/// "用户区原样保留"的另一面：模板改了**用户区自带的那段文案**时，已经装过的机器拿不到——
/// 升级只换托管区，用户区停在你装它那天。踩到过：改了 `USER_BLOCK` 的措辞，
/// 本机 `install` 一遍还是老话。README 第 5 节据此写的，别让它悄悄过期。
#[test]
fn install_never_refreshes_an_existing_user_block() {
    let home = tempfile::tempdir().unwrap();
    let skill = home.path().join(".claude/skills/zloop/SKILL.md");
    let user_region = |t: &str| t[t.find(hosts::USER_MARK).unwrap()..].to_string();
    let tpl = hosts::skill_markdown("claude");

    // 装过的机器 = 当前托管区 + 老模板那段用户区
    let old_user = "<!-- zloop:user -->\n<!-- 老模板当年写的那句 -->\n";
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, format!("{}{old_user}", &tpl[..tpl.find(hosts::USER_MARK).unwrap()])).unwrap();

    let w = hosts::install_claude(home.path(), false).unwrap();
    assert!(w[0].kept_user > 0, "{w:?}");
    let text = fs::read_to_string(&skill).unwrap();
    assert_eq!(user_region(&text), old_user, "老机器的用户区一个字都不该动");
    assert_ne!(user_region(&text), user_region(&tpl), "所以模板自带的那段只有全新安装才看得到");
}

/// 「目标存在但一条 todo 都没有」必须在 skill 的决策树里有自己的一支，
/// 而且三条命令都不能把它说成"已完成"——照"已完成 → goal new"那一支走，
/// 就会建出一个重复目标，把刚建的那个停放掉。(#5)
#[test]
fn skill_tells_you_to_plan_when_the_goal_has_no_todos() {
    // 0 待办时三条命令各自说什么——决策树里写的分辨方法必须跟实际输出对得上
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "还没规划的目标"], None, &[]);
    let status = zloop(d, &["status"], None, &[]).out;
    assert!(status.contains("待规划") && status.contains("还没有待办"), "{status}");
    let next: serde_json::Value = serde_json::from_str(&zloop(d, &["next", "--json"], None, &[]).out).unwrap();
    assert_eq!(next["reason"], "unplanned", "空清单要有自己的 reason，不能跟「全部完成」共用 all_done");
    assert_eq!(next["remaining"], 0);
    let ctx = zloop(d, &["context"], None, &[]).out;
    assert!(ctx.contains("还没有待办：先 zloop plan"), "待办那一节要直说下一步:\n{ctx}");
    assert!(!ctx.contains("全部完成"), "空目标的交接包里不许出现「全部完成」:\n{ctx}");
    assert!(ctx.contains("stopped (unplanned)") && ctx.contains("别新建目标"), "{ctx}");
    // start 也走同一个词，且给的是 plan 而不是 goal new
    let refused = zloop(d, &["start", "--fast"], None, &[]);
    assert_eq!(refused.code, 1, "{}{}", refused.out, refused.err);
    assert!(refused.err.contains("（unplanned）") && refused.err.contains("zloop plan"), "{}", refused.err);

    for host in ["claude", "codex-app"] {
        let text = hosts::skill_markdown(host);
        let branch = text.find("一条 todo 都没有").unwrap_or_else(|| panic!("{host} 模板缺「没有待办」这一支:\n{text}"));
        assert!(text.contains("待规划") && text.contains("不要 `goal new`"), "{text}");
        assert!(text.contains("unplanned") && text.contains("all_done"), "两个词要并排讲清，否则还是会读混:\n{text}");
        let trap = text.find(r#"`zloop goal new "$ARGUMENTS"`"#).expect("goal new 那一支还在");
        assert!(branch < trap, "新分支要排在「已完成 → goal new」前面，先读到的才管用");
    }

    // 验收要求的是"install 之后新模板里能看到"，所以照 install 的路径再验一遍
    let home = tempfile::tempdir().unwrap();
    hosts::install(true, true, false, home.path(), false).unwrap();
    for p in [".claude/skills/zloop/SKILL.md", ".codex/skills/zloop/SKILL.md"] {
        assert!(fs::read_to_string(home.path().join(p)).unwrap().contains("一条 todo 都没有"), "{p}");
    }
}

/// B-3：把待办一条条延后，最后一条延后完，整个目标就被当成"结束"了——`goal.status` 变
/// `done`、`status` 说"0 条待办全部完成，目标结束"、`start` 让人去 `goal new`。一条活都没做，
/// 出口却指向"丢掉这个目标"。走真命令（init → plan → edit --status deferred），不手搓状态。
#[test]
fn all_deferred_is_not_the_goal_finishing() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "全被推到以后的目标"], None, &[]);
    zloop(d, &["plan"], Some("[P0] a\n[P0] b\n"), &[]);
    for id in ["t1", "t2"] {
        let o = zloop(d, &["edit", id, "--status", "deferred"], None, &[]);
        assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    }

    // 目标没结束：一条都没完成，只是没活可跑
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.goal.status, "active", "延后最后一条不该把目标标成 done");

    let next: serde_json::Value = serde_json::from_str(&zloop(d, &["next", "--json"], None, &[]).out).unwrap();
    assert_eq!(next["reason"], "all_deferred", "全部延后要有自己的 reason，不能跟「全部完成」共用");
    assert_eq!(next["should_run"], false);

    let status = zloop(d, &["status", "--no-color"], None, &[]).out;
    assert!(!status.contains("目标结束"), "一条都没做完，别说目标结束:\n{status}");
    assert!(status.contains("全部延后") && status.contains("一条都没完成"), "{status}");
    assert!(status.contains("zloop edit t1 --status open"), "出口是把活捡回来，不是换目标:\n{status}");

    let ctx = zloop(d, &["context"], None, &[]).out;
    assert!(!ctx.contains("全部完成"), "交接包里不许出现「全部完成」:\n{ctx}");
    assert!(ctx.contains("all_deferred") && ctx.contains("别当成目标已完成"), "{ctx}");

    // start 也走同一个词，给的是"捡回来"而不是 goal new
    let refused = zloop(d, &["start", "--fast"], None, &[]);
    assert_eq!(refused.code, 1, "{}{}", refused.out, refused.err);
    assert!(refused.err.contains("（all_deferred）"), "{}", refused.err);
    assert!(refused.err.contains("--status open"), "{}", refused.err);
    assert!(!refused.err.contains("goal new"), "别引着人把没做的活丢掉: {}", refused.err);

    // 捡回来一条就该继续跑
    zloop(d, &["edit", "t1", "--status", "open"], None, &[]);
    let next: serde_json::Value = serde_json::from_str(&zloop(d, &["next", "--json"], None, &[]).out).unwrap();
    assert_eq!((&next["reason"], &next["todo"]["id"]), (&serde_json::json!("ready"), &serde_json::json!("t1")));

    // 有一条真做完了，就还是"目标结束"——这条路不受影响
    zloop(d, &["done", "t1", "--note", "ok", "--approach", "x"], None, &[]);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.goal.status, "done", "1 条完成 + 1 条延后：这才叫收工");
    assert!(zloop(d, &["status", "--no-color"], None, &[]).out.contains("目标结束"));

    // skill 模板也要把第三个词讲清，否则读的人还是照 all_done 那一支走
    for host in ["claude", "codex-app"] {
        let text = hosts::skill_markdown(host);
        assert!(text.contains("all_deferred"), "{host} 模板要提到 all_deferred:\n{text}");
    }
}

#[test]
fn hook_stop_blocks_only_when_runnable() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert_eq!(o.code, 0);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["decision"], "block");
    zloop(d, &["done", "t1", "--no-doc"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert_eq!(o.code, 0);
    assert_eq!(o.out, "");
}

#[test]
fn init_force_archives_the_previous_goal() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "first goal"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["done", "t1", "--note", "x", "--no-doc"], None, &[]);
    let o = zloop(d, &["init", "--force", "second goal"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("archived previous state → "), "{}", o.out);
    let archive = d.join(".zloop").join("archive");
    let files: Vec<_> = fs::read_dir(&archive).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    let old: serde_json::Value = serde_json::from_str(&fs::read_to_string(files[0].path()).unwrap()).unwrap();
    assert_eq!(old["goal"]["text"], "first goal");
    assert_eq!(old["ticks"].as_array().unwrap().len(), 1);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.goal.text, "second goal");
    assert!(st.todos.is_empty() && st.ticks.is_empty());
    // 第一个目标的日志文件还在磁盘上（只是不再算作当前目标的轮次）
    let kept = fs::read_dir(d.join(".zloop/log")).unwrap().flatten().count();
    assert!(kept > 0, "归档目标只是搬家，日志文件不能被删");
    let st = state::load(&state::state_path(d)).unwrap();
    let (rows, hidden) = zloop::log::entries(d, &st, None, 10).unwrap();
    assert!(rows.is_empty() && hidden == kept, "归档掉的目标的轮次不算当前目标的: {rows:?} hidden={hidden}");
}

#[test]
fn stale_in_progress_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["next"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.in_progress.as_mut().unwrap().started_at = "2026-08-27T00:00:00+08:00".into();
    state::save(&p, &mut st).unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("⚠ stale (>120m"));
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("⚠ 超过 120m 没动静"), "{}", o.out);
}

#[test]
fn plan_from_loopx_state_file() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    let f = d.join("ACTIVE_GOAL_STATE.md");
    fs::write(&f, "## Agent Todo\n\n- [x] [P1] done one\n- [ ] [P0] open one <!-- loopx:todo x=y -->\n").unwrap();
    let o = zloop(d, &["plan", "--from-loopx", f.to_str().unwrap()], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(o.out.trim(), "t1 [P0] open one");
}

/// A-11 端到端：机器时钟跳到未来一次，就足以让 runner 睡到下个世纪，而面板上一切正常。
///
/// 三处一起钉住（少任何一处，这一夜就是白跑的）：
/// 1. `next` 算出的等待封顶在配额窗口，不再是 38048610 分钟（72 年）；
/// 2. `status` 的「睡到 …」跨天带日期——只印 `00:00` 时它和正常的轮次间隔一模一样；
/// 3. `doctor` 直接说出"账本里有未来时间戳"，因为封顶只是不让它睡死，配额位还占着。
#[test]
fn a_clock_jump_into_the_future_is_capped_and_visible() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "clock"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    let o = zloop(d, &["done", "t1", "--note", "half", "--outcome", "progress", "--no-doc"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);

    // 真实世界的造法：先正常写下一轮，然后机器的钟跳了（NTP / 时区 / 挂起恢复）。
    // 配额设成 1，这条 tick 就把窗口占满了——而它在未来，窗口永远滑不过它。
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.policy.max_runs = 1;
    st.ticks.last_mut().unwrap().at = "2099-01-01T00:00:00+08:00".into();
    state::save(&p, &mut st).unwrap();

    let o = zloop(d, &["next", "--peek", "--json"], None, &[]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap_or_else(|e| panic!("{e}: {}{}", o.out, o.err));
    assert_eq!(v["reason"], "throttled", "{}", o.out);
    let minutes = v["interval_min"].as_u64().unwrap_or_else(|| panic!("{}", o.out));
    assert!(minutes <= 24 * 60, "等待要封顶在配额窗口内，撤掉封顶这里是 38048610 分钟（72 年）：{minutes}");

    // runner 会照这个间隔写下 sleep；status 读的就是这一条
    let j = d.join(".zloop").join("runner");
    fs::create_dir_all(&j).unwrap();
    let wake = chrono::Local::now() + chrono::Duration::minutes(minutes as i64);
    let until = wake.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    fs::write(
        j.join("journal.jsonl"),
        format!("{{\"event\":\"sleep\",\"until\":\"{until}\",\"reason\":\"throttled\",\"at\":\"x\"}}\n"),
    )
    .unwrap();
    let o = zloop(d, &["status"], None, &[]);
    let day = wake.format("%m-%d").to_string();
    assert!(o.out.contains("睡到"), "{}", o.out);
    assert!(o.out.contains(&day), "跨天的「睡到」必须带上日期，否则和正常的轮次间隔长得一模一样：{}", o.out);
    assert!(zloop(d, &["context"], None, &[]).out.contains(&day), "机器读的那句同样要带日期");

    // 封顶只是不让它睡死：那条 tick 还占着配额位，得有人来看一眼
    let o = zloop(d, &["doctor", "--json"], None, &[]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    let kinds: Vec<&str> = v["findings"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
    assert!(kinds.contains(&"future_timestamp"), "doctor 要报出未来时间戳：{}", o.out);
    assert_eq!((v["errors"].as_u64(), o.code), (Some(1), 1), "光这条 tick 就占满了配额，循环已经限流住了：{}", o.out);
}

#[test]
fn phase_tracks_the_round() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    // 完整的 phase 句子是 `zloop context` 的契约；status 只显示压缩版，改样式不该动契约。
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run t1"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("就绪"));
    zloop(d, &["next", "--peek"], None, &[]);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run"));
    let o = zloop(d, &["next", "--json"], None, &[("CLAUDE_CODE_SESSION_ID", "sess-p")]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert!(v["phase"].as_str().unwrap().starts_with("executing t1 · round 1"), "{}", v["phase"]);
    assert!(v.as_object().unwrap().len() <= 10);
    let st = state::load(&state::state_path(d)).unwrap();
    let ip = st.in_progress.as_ref().unwrap();
    assert_eq!(
        (ip.todo.as_str(), ip.via.as_str(), ip.host.as_deref(), ip.session.as_deref()),
        ("t1", "next", Some("claude"), Some("sess-p"))
    );
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("执行中") && o.out.contains("claude 正在做 t1") && o.out.contains("第 1 轮"), "{}", o.out);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：executing t1"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("host claude · via next"));
    zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    assert!(state::load(&state::state_path(d)).unwrap().in_progress.is_none());
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run t2"));
    zloop(d, &["done", "t2", "--block", "?"], None, &[]);
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：waiting (user_gate) · retry in 10 min"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("等你回答 · 10 分钟后重试"));
    for _ in 0..3 {
        zloop(d, &["next"], None, &[]);
    }
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：stopped (user_gate)"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("等你决定"));
    let j = d.join(".zloop").join("runner");
    fs::create_dir_all(&j).unwrap();
    let until = (chrono::Local::now() + chrono::Duration::minutes(5)).to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    fs::write(
        j.join("journal.jsonl"),
        format!("{{\"event\":\"sleep\",\"until\":\"{until}\",\"reason\":\"ready\",\"at\":\"x\"}}\n"),
    )
    .unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("runner sleeping until"));
    // 此刻所有 todo 都在等人回话，所以标题让位给「等你决定」，休眠时间退到明细行。
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("等你决定") && o.out.contains("睡到"), "{}", o.out);
    zloop(d, &["edit", "t2", "--status", "open"], None, &[]);
    assert!(zloop(d, &["status"], None, &[]).out.contains("休眠中"), "有活可干时才轮到休眠当标题");
    fs::write(
        j.join("journal.jsonl"),
        "{\"event\":\"begin\",\"round\":4,\"todo\":\"t2\",\"host\":\"claude\",\"at\":\"2026-08-27T00:00:00+08:00\"}\n",
    )
    .unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("runner round 4 on t2"));
    assert!(zloop(d, &["status"], None, &[]).out.contains("第 4 轮做 t2"), "{}", zloop(d, &["status"], None, &[]).out);
}

#[test]
fn spawned_zloop_never_inherits_the_ambient_session() {
    // Guards `cargo test` run from inside a host session or from `zloop run` itself: if any of
    // these leak through, tests silently exercise a different code path (`hook-stop` goes quiet,
    // ticks get stamped with the outer session id) and the suite is red for no real reason.
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", r#"printf '%s' "${ZLOOP_RUNNER-_}${CLAUDECODE-_}${CLAUDE_CODE_SESSION_ID-_}${CODEX_THREAD_ID-_}""#]);
    cmd.env("ZLOOP_RUNNER", "1")
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "sess")
        .env("CODEX_THREAD_ID", "thread");
    common::scrub_ambient_env(&mut cmd);
    let o = cmd.output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), "____", "scrub_ambient_env 漏掉了一个变量");
}

#[test]
fn the_stop_hook_defers_to_whoever_already_took_the_round() {
    // `next` 早就挡了「别人正拿着这一轮」，但 hook 一直没走这道闸，于是
    // 「next 说不给你」和「hook 催你去做」同时成立——人照着 hook 敲下去就撞了。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let a = [("CLAUDE_CODE_SESSION_ID", "会话A")];
    let b = [("CLAUDE_CODE_SESSION_ID", "会话B")];

    // A 领走这一轮
    let o = zloop(d, &["next", "--json"], None, &a);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&o.out).unwrap()["should_run"], true, "{}", o.out);

    // B 问 next：被挡（这是原有行为，先钉住）
    let o = zloop(d, &["next", "--json"], None, &b);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&o.out).unwrap()["reason"], "held_by_other", "{}", o.out);

    // B 的 hook：也该闭嘴，不能跟 next 说两套话
    let o = zloop(d, &["hook-stop"], Some("{}"), &b);
    assert_eq!((o.code, o.out.as_str()), (0, ""), "next 不给 B 派活，hook 就不能催 B 去做");

    // A 自己的 hook：照常催——活本来就是它的
    let o = zloop(d, &["hook-stop"], Some("{}"), &a);
    assert!(o.out.contains("\"block\""), "拿着活的那个会话不该被自己挡住: {}", o.out);

    // 裸 CLI（没有 session id）：不被误锁，行为不变
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert!(o.out.contains("\"block\""), "分不出是谁就不该拦，否则把人锁在门外: {}", o.out);
}

#[test]
fn hook_stop_passes_through_under_runner() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["hook-stop"], Some("{}"), &[("ZLOOP_RUNNER", "1")]);
    assert_eq!((o.code, o.out.as_str()), (0, ""));
    let o = zloop(d, &["hook-stop"], Some("{}"), &[]);
    assert!(o.out.contains("\"block\""));
}

#[test]
fn acceptance_shows_up_and_done_without_evidence_hints() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    let o = zloop(d, &["plan", "--add", "[P0] ship :: tests green"], None, &[]);
    assert_eq!(o.out.trim(), "t1 [P0] ship :: tests green");
    let o = zloop(d, &["next", "--json"], None, &[]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!(v["todo"]["acceptance"], "tests green");
    assert!(zloop(d, &["status"], None, &[]).out.contains("验收：tests green"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("验收：tests green"));
    let o = zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    assert!(o.out.contains("hint: t1 有验收标准"), "{}", o.out);
    zloop(d, &["plan", "--add", "[P0] b"], None, &[]);
    zloop(d, &["edit", "t2", "--acceptance", "lint passes"], None, &[]);
    let o = zloop(d, &["done", "t2", "--note", "ok", "--evidence", "lint output clean", "--no-doc"], None, &[]);
    assert!(!o.out.contains("有验收标准"), "evidence given → no acceptance hint: {}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    let (logs, _) = zloop::log::entries(d, &st, Some("t2"), 5).unwrap();
    assert!(fs::read_to_string(&logs[0].0).unwrap().contains("- acceptance: lint passes"));
}

#[test]
fn status_shows_spend_and_notify_cmd_receives_events() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let o = zloop(d, &["notify"], None, &[]);
    assert_eq!(o.code, 2, "nothing configured yet");
    zloop(d, &["done", "t1", "--outcome", "progress", "--note", "x"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.ticks[0].cost_usd = Some(0.25);
    st.policy.max_total_usd = 2.0;
    st.policy.notify_cmd = Some(format!("cat >> {}", d.join("notify.log").display()));
    state::save(&p, &mut st).unwrap();
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("花了 $0.25（上限 $2.00）"), "{}", o.out);
    assert!(zloop(d, &["context"], None, &[]).out.contains("已花费：$0.25 / 上限 $2.00"));
    let o = zloop(d, &["notify", "hello there"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let log = fs::read_to_string(d.join("notify.log")).unwrap();
    assert!(log.contains("hello there") && log.contains("\"event\":\"test\""), "{log}");
}

#[test]
fn remember_pause_resume_and_compact() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    // remember → NOTES.md → context
    let o = zloop(d, &["remember", "run cargo test before done; the fmt check is flaky"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(d.join(".zloop/NOTES.md").exists());
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("## 经验") && o.out.contains("fmt check is flaky"), "{}", o.out);
    // pause / resume
    let o = zloop(d, &["pause"], None, &[]);
    assert!(o.out.contains("paused"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：stopped (paused)"));
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("已暂停") && o.out.contains("zloop resume"), "{}", o.out);
    let o = zloop(d, &["resume"], None, &[]);
    assert!(o.out.contains("active"));
    assert!(zloop(d, &["context"], None, &[]).out.contains("阶段：idle · next would run"));
    // compact: nothing old yet
    let o = zloop(d, &["compact"], None, &[]);
    assert!(o.out.contains("nothing to compact"));
    zloop(d, &["done", "t1", "--note", "old work", "--no-doc"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.todos[0].done_at = Some("2026-01-01T00:00:00+08:00".into());
    st.ticks[0].at = "2026-01-01T00:00:00+08:00".into();
    state::save(&p, &mut st).unwrap();
    let o = zloop(d, &["compact", "--keep-days", "30"], None, &[]);
    assert!(o.out.contains("compacted 1 todos and 1 ticks"), "{}", o.out);
    let st = state::load(&p).unwrap();
    assert_eq!(st.todos.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2"]);
    assert!(st.ticks.is_empty());
    let archives: Vec<_> = fs::read_dir(d.join(".zloop/archive")).unwrap().flatten().collect();
    assert_eq!(archives.len(), 1);
    let a: serde_json::Value = serde_json::from_str(&fs::read_to_string(archives[0].path()).unwrap()).unwrap();
    assert_eq!(a["todos"][0]["id"], "t1");
    // t2 still runnable; the goal stays active
    assert!(zloop(d, &["next", "--peek", "--json"], None, &[]).out.contains("\"id\": \"t2\""));
}

/// A-18：整理账本不能顺手给预算闸提额。
///
/// `compact` 把老 todo 名下的 tick 搬进 `archive/`，而花费就记在 tick 上。少了累计汇总，
/// 一个已经撞到 `policy.max_total_usd` 停下的目标，被例行整理一次就回到 ready——
/// 而且 `status` 连花过钱这件事都不再显示，人不知道自己刚给循环提了额。
/// （复现脚本：`scripts/repro-a18-compact-resets-budget-cap.sh`）
#[test]
fn compacting_the_ledger_does_not_disarm_the_budget_cap() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 老活", "--add", "[P1] 还没做的活"], None, &[]);
    zloop(d, &["done", "t1", "--note", "老活做完", "--no-doc"], None, &[]);

    // 一个跑了一个月、已经花超上限的目标：t1 那一轮花了 $9.50，上限 $5.00
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.policy.max_total_usd = 5.0;
    st.todos[0].done_at = Some("2026-01-01T00:00:00+08:00".into());
    st.ticks[0].at = "2026-01-01T00:00:00+08:00".into();
    st.ticks[0].cost_usd = Some(9.5);
    state::save(&p, &mut st).unwrap();

    let peek = |d: &Path| zloop(d, &["next", "--peek", "--json"], None, &[]).out;
    assert!(peek(d).contains("\"reason\": \"budget\""), "前提没成立：整理之前就该是 budget\n{}", peek(d));

    let o = zloop(d, &["compact", "--keep-days", "30"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("compacted 1 todos and 1 ticks"), "{}", o.out);
    // 搬走了一笔钱就要说出来
    assert!(o.out.contains("$9.50"), "整理带走了花费却不吭声：{}", o.out);

    // tick 归档走了，账没走
    let st = state::load(&p).unwrap();
    assert!(st.ticks.is_empty(), "tick 应该被搬走：{:?}", st.ticks);
    assert_eq!(st.archived.ticks, 1);
    assert!((st.archived.cost_usd - 9.5).abs() < 1e-9, "{:?}", st.archived);
    assert!((zloop::tick::spent_total(&st) - 9.5).abs() < 1e-9);

    // 闸还在：调度器、start 的预检、status 三处都还看得见这 $9.50
    assert!(peek(d).contains("\"reason\": \"budget\""), "整理之后预算闸没了：{}", peek(d));
    let o = zloop(d, &["start", "--host", "claude", "--fast"], None, &[]);
    assert!(o.out.contains("budget") || o.err.contains("budget"), "{}{}", o.out, o.err);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("$9.50") && o.out.contains("上限 $5.00"), "status 不再提花过的钱：{}", o.out);
    // 再整理一次不会把同一笔钱记两遍
    zloop(d, &["compact", "--keep-days", "30"], None, &[]);
    let st = state::load(&p).unwrap();
    assert!((st.archived.cost_usd - 9.5).abs() < 1e-9, "{:?}", st.archived);
}

/// A-18 的另一半：`compact` 动的是 runner 下一轮要读的账，所以和 `goal switch` 走同一道闸。
#[test]
fn compacting_is_refused_while_the_runner_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.todos[0].done_at = Some("2026-01-01T00:00:00+08:00".into());
    st.ticks[0].at = "2026-01-01T00:00:00+08:00".into();
    state::save(&p, &mut st).unwrap();

    // runner 在跑（pid 文件指向一个活着的进程）
    fs::create_dir_all(d.join(".zloop/runner")).unwrap();
    fs::write(d.join(".zloop/runner/pid"), format!("{}\n", std::process::id())).unwrap();
    let o = zloop(d, &["compact", "--keep-days", "30"], None, &[]);
    assert_ne!(o.code, 0, "runner 跑着还让整理：{}{}", o.out, o.err);
    assert!(o.err.contains("runner 正在跑"), "{}", o.err);
    assert!(!state::load(&p).unwrap().ticks.is_empty(), "被拒的 compact 不许动账本");
    // --force 才放行
    let o = zloop(d, &["compact", "--keep-days", "30", "--force"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("compacted 1 todos"), "{}", o.out);
    fs::remove_file(d.join(".zloop/runner/pid")).unwrap();

    // 有会话拿着 todo 没写回，同样先别动
    zloop(d, &["next"], None, &[]);
    let o = zloop(d, &["compact", "--keep-days", "30"], None, &[]);
    assert_ne!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.err.contains("还没写回"), "{}", o.err);
}

// ---------- 每轮技术文档 ----------

#[test]
fn done_refuses_to_finish_without_a_technical_document() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b", "--add", "[P2] c"], None, &[]);

    // finishing without --approach is rejected, and nothing is written
    let o = zloop(d, &["done", "t1", "--note", "ok"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("需要留下技术文档"), "{}", o.err);
    assert!(o.err.contains("--approach") && o.err.contains("--pitfall") && o.err.contains("--no-doc"), "{}", o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.todos[0].status, "open", "rejected call must not change state");
    assert!(st.ticks.is_empty());

    // progress / block 不欠"实现思路"——没做完的轮次谈不上"怎么做的"
    let o = zloop(d, &["done", "t1", "--outcome", "progress", "--note", "half"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    // fail 欠的是另一样东西：踩到的坑（policy.require_pitfall，见 a_failed_round_must_leave_a_pitfall…）
    let o = zloop(d, &["done", "t1", "--outcome", "fail", "--note", "boom", "--pitfall", "链接器缺符号"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let o = zloop(d, &["done", "t3", "--block", "which db?"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);

    // with the document it goes through and every section is rendered
    let o = zloop(
        d,
        &[
            "done",
            "t1",
            "--note",
            "done at last",
            "--approach",
            "先量基线再改：bench.sh 跑 3 次取中位数，只对最慢的一步做懒加载",
            "--decision",
            "不引入缓存层，成本高于收益",
            "--decision",
            "懒加载放在入口而不是每个 use 处",
            "--pitfall",
            "release 与 debug 差 3 倍，基线必须用 release",
            "--evidence",
            "cargo test 64 passed",
        ],
        None,
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(!o.out.contains("hint: 这一轮没有实现思路"), "{}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    let last = st.ticks.last().unwrap();
    assert_eq!((last.outcome.as_str(), last.documented), ("done", Some(true)));
    let body = fs::read_to_string(d.join(".zloop").join(last.log.as_deref().unwrap())).unwrap();
    assert!(body.contains("## 实现思路") && body.contains("bench.sh 跑 3 次取中位数"), "{body}");
    assert!(body.contains("## 关键决策") && body.contains("- 不引入缓存层") && body.contains("- 懒加载放在入口"), "{body}");
    assert!(body.contains("## 遇到的坑") && body.contains("release 与 debug 差 3 倍"), "{body}");
    assert!(body.contains("## 验证证据") && body.contains("cargo test 64 passed"), "{body}");
    assert!(!body.contains("⚠ 这一轮没有留下实现思路"), "{body}");
}

#[test]
fn no_doc_escape_hatch_is_marked_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    let o = zloop(d, &["done", "t1", "--note", "trivial", "--no-doc"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("hint: 这一轮没有实现思路"), "{}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.ticks.last().unwrap().documented, Some(false));
    let body = fs::read_to_string(d.join(".zloop").join(st.ticks.last().unwrap().log.as_deref().unwrap())).unwrap();
    assert!(body.contains("⚠ 这一轮没有留下实现思路"), "{body}");
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains("⚠ .zloop/log/"), "{}", o.out);
    assert!(o.out.contains("只有结果记录"), "{}", o.out);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("1 轮缺实现思路"), "{}", o.out);
    // policy off → plain done works again
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.policy.require_doc = false;
    state::save(&p, &mut st).unwrap();
    assert_eq!(zloop(d, &["done", "t2", "--note", "no policy"], None, &[]).code, 0);
}

#[test]
fn doc_assembles_rounds_into_one_document() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把启动时间降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 量基线 :: bench.sh 连跑 3 次", "--add", "[P1] 懒加载"], None, &[]);
    zloop(
        d,
        &["done", "t1", "--outcome", "progress", "--note", "第一步", "--approach", "先写 bench.sh"],
        None,
        &[("CLAUDE_CODE_SESSION_ID", "sess-doc")],
    );
    zloop(
        d,
        &["done", "t1", "--note", "基线 3.2s", "--approach", "取中位数避免抖动", "--pitfall", "debug 模式差 3 倍"],
        None,
        &[("CLAUDE_CODE_SESSION_ID", "sess-doc")],
    );
    zloop(d, &["done", "t2", "--note", "懒加载完成", "--no-doc"], None, &[]);

    let o = zloop(d, &["doc", "t1"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.starts_with("# 技术文档 · "), "{}", o.out);
    assert!(o.out.contains("**目标**：把启动时间降到 1 秒"));
    assert!(o.out.contains("## t1 [P0] 量基线"));
    assert!(o.out.contains("- 验收标准：bench.sh 连跑 3 次"));
    assert_eq!(o.out.matches("### 轮次").count(), 2, "both rounds of t1: {}", o.out);
    assert!(o.out.contains("#### 实现思路"), "sections demoted under the round: {}", o.out);
    assert!(o.out.contains("取中位数避免抖动") && o.out.contains("debug 模式差 3 倍"));
    assert!(o.out.contains("claude --resume sess-doc"), "resume command carried over: {}", o.out);
    assert!(!o.out.contains("## t2 "), "only the requested todo");

    // --all covers every todo and flags the undocumented round
    let out_file = d.join("docs").join("TECH.md");
    let o = zloop(d, &["doc", "--all", "--out", out_file.to_str().unwrap()], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("wrote ") && o.out.contains("2 条 todo"), "{}", o.out);
    let text = fs::read_to_string(&out_file).unwrap();
    assert!(text.contains("## t1 [P0]") && text.contains("## t2 [P1]"));
    assert!(text.contains("（这一轮没有实现思路，只有结果记录）"), "{text}");

    assert_eq!(zloop(d, &["doc", "t99"], None, &[]).code, 2);
    assert_eq!(zloop(d, &["doc"], None, &[]).code, 2);
}

#[test]
fn doc_range_takes_recent_rounds_or_a_time_window() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把启动时间降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 量基线", "--add", "[P1] 懒加载", "--add", "[P2] 收尾"], None, &[]);
    for id in ["t1", "t2", "t3"] {
        let o = zloop(d, &["done", id, "--note", "ok", "--approach", &format!("{id} 的思路")], None, &[]);
        assert_eq!(o.code, 0, "{}", o.err);
    }
    // 三轮全落在同一秒里，时间窗口就没得测：把它们摊到三天上。
    // tick.log 记的是文件路径，改 at 不影响 assemble 取正文。
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    assert_eq!(st.ticks.len(), 3, "一条 done 一轮，多出来的 tick 会让下面的时间对不上号");
    let days = ["2026-08-01T10:00:00+08:00", "2026-08-15T10:00:00+08:00", "2026-08-29T10:00:00+08:00"];
    for (tick, at) in st.ticks.iter_mut().zip(days) {
        tick.at = at.into();
    }
    state::save(&p, &mut st).unwrap();

    // 默认行为不变：三章都在，一个字的范围提示都不该出现
    let o = zloop(d, &["doc", "--all"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert_eq!(o.out.matches("\n## t").count(), 3, "{}", o.out);
    assert!(!o.out.contains("**范围**"), "不带范围参数就出全文，不加抬头: {}", o.out);

    // --last N：只留最近 N 轮；范围外的 todo 整章不出，并如实交代省了几轮
    let o = zloop(d, &["doc", "--all", "--last", "1"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("**范围**：最近 1 轮 —— 收录 1 轮，省略 2 轮"), "{}", o.out);
    assert_eq!(o.out.matches("\n## t").count(), 1, "空章不占版面: {}", o.out);
    assert!(o.out.contains("## t3 ") && !o.out.contains("## t1 "), "留下的是最近那轮: {}", o.out);

    // --since / --until：两头都认，合起来是个闭区间
    let o = zloop(d, &["doc", "--all", "--since", "2026-08-10"], None, &[]);
    assert!(o.out.contains("## t2 ") && o.out.contains("## t3 ") && !o.out.contains("## t1 "), "{}", o.out);
    let o = zloop(d, &["doc", "--all", "--until", "2026-08-10"], None, &[]);
    assert!(o.out.contains("## t1 ") && !o.out.contains("## t2 "), "{}", o.out);
    let o = zloop(d, &["doc", "--all", "--since", "2026-08-10", "--until", "2026-08-20"], None, &[]);
    assert_eq!(o.out.matches("\n## t").count(), 1, "{}", o.out);
    assert!(o.out.contains("## t2 "), "{}", o.out);

    // 单条 todo 一样能限范围；窗口里一轮都没有时说清楚，而不是装作没这条 todo
    let o = zloop(d, &["doc", "t1", "--since", "2026-08-10"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("收录 0 轮，省略 1 轮"), "{}", o.out);

    // --out 报的是真写进去的章数，不是 --all 的总数
    let out_file = d.join("TECH.md");
    let o = zloop(d, &["doc", "--all", "--last", "2", "--out", out_file.to_str().unwrap()], None, &[]);
    assert!(o.out.contains("2 条 todo"), "{}", o.out);
    assert_eq!(fs::read_to_string(&out_file).unwrap().matches("\n## t").count(), 2);

    // 看不懂的时间、以及空区间，都要拦在出文档之前
    let o = zloop(d, &["doc", "--all", "--since", "上周二"], None, &[]);
    assert_eq!(o.code, 2, "{}", o.out);
    assert!(o.err.contains("看不懂的时间"), "{}", o.err);
    let o = zloop(d, &["doc", "--all", "--since", "2026-08-29", "--until", "2026-08-01"], None, &[]);
    assert_eq!(o.code, 2, "{}", o.out);
    assert!(o.err.contains("空的"), "{}", o.err);

    // 相对写法（`--since 1d`）要被解成一个具体时刻再去筛，抬头上写的是解出来的时间戳。
    // 收录几轮取决于今天是哪天，所以这里只断言它解开了，不断言条数。
    let o = zloop(d, &["doc", "--all", "--since", "1d"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    assert!(o.out.contains("之后 —— 收录") && !o.out.contains("1d 之后"), "{}", o.out);
}

#[test]
fn changed_files_are_captured_from_git() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(d).output().unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    fs::write(d.join("keep.txt"), "one\n").unwrap();
    fs::write(d.join(".gitignore"), ".zloop/\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "init"]);

    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    fs::write(d.join("keep.txt"), "one\ntwo\n").unwrap(); // modified
    fs::write(d.join("brand-new.rs"), "fn main() {}\n").unwrap(); // untracked
    let o = zloop(d, &["done", "t1", "--note", "touched files", "--approach", "改了两个文件"], None, &[]);
    assert_eq!(o.code, 0, "{}", o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    let body = fs::read_to_string(d.join(".zloop").join(st.ticks[0].log.as_deref().unwrap())).unwrap();
    assert!(body.contains("## 改动文件"), "{body}");
    assert!(body.contains("keep.txt"), "modified file listed: {body}");
    assert!(body.contains("brand-new.rs (new)"), "untracked file listed: {body}");
    assert!(!body.contains(".zloop/"), "zloop's own state must not be listed: {body}");
}

// ---------- status 的观感 ----------

#[test]
fn status_headline_names_the_state_and_colour_is_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把启动时间降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);

    // 就绪：有活可做
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("就绪"), "{}", o.out);
    assert!(o.out.contains("░"), "progress bar: {}", o.out);
    assert!(o.out.contains("目标") && o.out.contains("把启动时间降到 1 秒"), "目标单独一行: {}", o.out);
    // 清单是一张表：步骤（执行顺序）/ id（命令里敲的）/ 这一步做什么 / 进展
    assert!(o.out.contains("清单") && o.out.contains("0/2 完成"), "清单进度: {}", o.out);
    for h in ["步骤", "id", "这一步做什么", "进展"] {
        assert!(o.out.contains(h), "表头缺 {h}: {}", o.out);
    }
    assert!(o.out.contains('┌') && o.out.contains('┼') && o.out.contains('┘'), "画出框线: {}", o.out);
    // id 每一行都要有——做完的那些以前不显示，看的人只能靠数行猜
    assert!(o.out.contains("│ t1 │") && o.out.contains("│ t2 │"), "每行都带 id: {}", o.out);
    assert!(o.out.contains("▶ 下一个") && o.out.contains("○ 排队中"), "每一步自己说清进展: {}", o.out);
    // 表格每一行宽度必须一致，否则右边框会歪
    let widths: Vec<usize> =
        o.out.lines().filter(|l| l.contains('│') || l.contains('┌') || l.contains('└')).map(zloop::style::width).collect();
    assert!(widths.windows(2).all(|w| w[0] == w[1]), "表格各行宽度不齐: {widths:?}");
    assert!(o.out.contains("开跑") && o.out.contains("zloop start"), "next action spelled out: {}", o.out);
    assert!(!o.out.contains('\u{1b}'), "piped output carries no escape codes: {:?}", o.out);

    // 不换行才是关键：折行会丢掉左边的槽位，那正是“乱”的来源。
    for cols in [46usize, 60, 80, 100] {
        let o = zloop(d, &["status"], None, &[("COLUMNS", &cols.to_string())]);
        for line in o.out.lines() {
            assert!(zloop::style::width(line) <= cols, "{cols} 列下这行超宽 ({}): {line:?}", zloop::style::width(line));
        }
    }

    // 管道无色，CLICOLOR_FORCE 有色，--no-color 强制无色
    let forced = zloop(d, &["status"], None, &[("CLICOLOR_FORCE", "1")]);
    assert!(forced.out.contains('\u{1b}'), "CLICOLOR_FORCE=1 colourises: {:?}", forced.out);
    let off = zloop(d, &["status", "--no-color"], None, &[("CLICOLOR_FORCE", "1")]);
    assert!(!off.out.contains('\u{1b}'), "--no-color wins: {:?}", off.out);
    let no_color_env = zloop(d, &["status"], None, &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]);
    assert!(!no_color_env.out.contains('\u{1b}'), "NO_COLOR wins: {:?}", no_color_env.out);

    // 等你决定
    zloop(d, &["done", "t1", "--block", "用哪个库？"], None, &[]);
    zloop(d, &["done", "t2", "--block", "要上线吗？"], None, &[]);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("等你决定"), "{}", o.out);
    assert!(o.out.contains("↳ 用哪个库？"), "the blocking question is shown inline: {}", o.out);
    assert!(
        o.out.contains("等你回话") && o.out.contains("答完敲 zloop edit t1 --status open"),
        "解锁命令贴在那条 todo 自己下面: {}",
        o.out
    );
    assert!(o.out.contains("答完敲 zloop edit t2 --status open"), "每条被挡住的都有自己的命令: {}", o.out);
    // 被 --block 的轮次不欠文档
    assert!(!o.out.contains("只有结果记录"), "block rounds owe no document: {}", o.out);

    // 已暂停
    zloop(d, &["pause"], None, &[]);
    assert!(zloop(d, &["status"], None, &[]).out.contains("已暂停"));
    zloop(d, &["resume"], None, &[]);

    // 完成
    zloop(d, &["edit", "t1", "--status", "open"], None, &[]);
    zloop(d, &["edit", "t2", "--status", "open"], None, &[]);
    zloop(d, &["done", "t1", "--note", "x", "--approach", "怎么做的"], None, &[]);
    zloop(d, &["done", "t2", "--note", "y", "--approach", "怎么做的"], None, &[]);
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("完成"), "{}", o.out);
    assert!(o.out.contains("2/2 完成") && o.out.contains("100%"), "{}", o.out);
    // 做完的步骤要留在清单上打勾——「做过哪几步」是复盘时最想看的
    assert_eq!(o.out.matches('✅').count(), 3, "标题一个 ✅ + 两步各一个: {}", o.out);
    assert!(o.out.contains("│ t1 │") && o.out.contains("│ t2 │"), "完成后清单还在: {}", o.out);
    // 换目标走 goal new（停放旧的、可切回），不再是 init --force（归档、切不回来）
    assert!(o.out.contains("zloop plan --add") && o.out.contains("zloop goal new"), "what to do next: {}", o.out);
    assert!(!o.out.contains("init --force"), "别再教用户覆盖目标: {}", o.out);
    assert!(o.out.contains("zloop doc --all"), "and how to collect the documents: {}", o.out);
    assert!(!o.out.contains('░'), "a finished bar is entirely full: {}", o.out);
}

// ---------- 多目标 ----------

#[test]
fn goals_park_switch_and_archive_without_losing_anything() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把冷启动降到 1 秒"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 找最慢的三处", "--add", "[P1] 加 tracing"], None, &[]);
    zloop(d, &["done", "t1", "--note", "定位到 3 处", "--approach", "tracing 打点"], None, &[]);

    // 新目标：旧的原地停放，不是覆盖
    let o = zloop(d, &["goal", "new", "让 keep-awake 支持外接显示器"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("停放") && o.out.contains("zloop goal switch"), "{}", o.out);
    assert!(o.out.contains("新目标"), "{}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!((st.goal.text.as_str(), st.todos.len()), ("让 keep-awake 支持外接显示器", 0), "新目标是干净的");
    assert_eq!(st.goal.id, "keep-awake", "id 从目标文字里的英文词取: {}", st.goal.id);

    // 两个都在，当前那个带 ▸
    let o = zloop(d, &["goal", "list"], None, &[]);
    assert!(o.out.contains("共 2 个目标"), "{}", o.out);
    assert!(o.out.contains("▸ keep-awake") && o.out.contains("让 keep-awake 支持外接显示器"), "{}", o.out);
    assert!(o.out.contains("把冷启动降到 1 秒"), "停放的也列出来: {}", o.out);
    // status 里能看见还有别的目标
    assert!(zloop(d, &["status"], None, &[]).out.contains("另有 1 个目标停着"), "{}", o.out);
    // 空目标不说「全部完成」
    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains("待规划") && o.out.contains("还没有待办"), "{}", o.out);

    // 用目标文字的片段切回去，进度一条不少
    let o = zloop(d, &["goal", "switch", "冷启动"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.goal.text, "把冷启动降到 1 秒");
    assert_eq!(st.todos.len(), 2);
    assert_eq!(st.todos[0].status, "done");
    assert_eq!(st.ticks.len(), 1, "tick 账本跟着目标走");
    assert!(zloop(d, &["status"], None, &[]).out.contains("1/2 完成"), "步骤进度还在");

    // 归档：从 list 里消失，文件搬到 archive/
    let o = zloop(d, &["goal", "rm", "keep-awake"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("已归档"), "{}", o.out);
    assert!(!zloop(d, &["goal", "list"], None, &[]).out.contains("keep-awake"));
    let archived: Vec<_> = fs::read_dir(d.join(".zloop/archive")).unwrap().flatten().collect();
    assert_eq!(archived.len(), 1, "归档只是搬家，不是删除");
    // 当前目标不能被归档
    let o = zloop(d, &["goal", "rm", "冷启动"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("是当前目标"), "{}", o.err);
}

#[test]
fn switching_goals_is_refused_while_work_is_in_flight() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "目标一"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["goal", "new", "目标二"], None, &[]);
    zloop(d, &["goal", "switch", "目标一"], None, &[]);

    // 有会话拿着 todo 没写回
    zloop(d, &["next"], None, &[]);
    let o = zloop(d, &["goal", "switch", "目标二"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("还没写回"), "{}", o.err);
    // --force 才放行
    assert_eq!(zloop(d, &["goal", "switch", "目标二", "--force"], None, &[]).code, 0);
    zloop(d, &["goal", "switch", "目标一", "--force"], None, &[]);

    // runner 在跑（pid 文件指向一个活着的进程）
    zloop(d, &["done", "t1", "--note", "ok", "--no-doc"], None, &[]);
    fs::create_dir_all(d.join(".zloop/runner")).unwrap();
    fs::write(d.join(".zloop/runner/pid"), format!("{}\n", std::process::id())).unwrap();
    let o = zloop(d, &["goal", "switch", "目标二"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("runner 正在跑"), "{}", o.err);
    fs::remove_file(d.join(".zloop/runner/pid")).unwrap();

    // 片段对上多个目标时要求说清楚
    zloop(d, &["goal", "new", "目标三"], None, &[]);
    let o = zloop(d, &["goal", "switch", "目标"], None, &[]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("对上了 3 个目标"), "{}", o.err);
}

// ---------- 多目标：搬家事务与派活归属（GOALS-REVIEW.md 的 F1–F7 / L1–L2） ----------

/// 被拒的 `goal new` 一定不能把当前目标停走：校验在 park 之前，失败要回滚。
#[test]
fn a_rejected_goal_new_leaves_the_current_goal_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "原来的目标"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    let path = state::state_path(d);

    // --id 里没有可用字符
    let o = zloop(d, &["goal", "new", "新的", "--id", "中文标题"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(path.is_file(), "旧目标必须还在 state.json 里，不能停走后才报错");
    assert_eq!(state::load(&path).unwrap().goal.text, "原来的目标");

    // --id 撞了当前目标自己的 id（它马上要停到 goals/<id>.json 去）
    let cur_id = state::load(&path).unwrap().goal.id;
    let o = zloop(d, &["goal", "new", "新的", "--id", &cur_id], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("已经有人用了"), "{}", o.err);
    assert!(path.is_file());

    // 别人持锁：park 也在锁内，所以拿不到锁时一个文件都不该动
    zloop::state::locked(&path, std::time::Duration::from_secs(30), || {
        let o = zloop(d, &["goal", "new", "抢锁的目标"], None, &[]);
        assert_ne!(o.code, 0, "拿不到锁应该失败: {}{}", o.out, o.err);
        assert!(path.is_file(), "锁超时不能把当前目标吞掉（headless）");
        Ok(())
    })
    .unwrap();
    assert_eq!(state::load(&path).unwrap().goal.text, "原来的目标");
    assert_eq!(zloop(d, &["goal", "list"], None, &[]).out.matches("共 1 个目标").count(), 1);
}

/// 目标 id 的清单，`goal list --json` 里的顺序（当前目标在最前）。
fn goal_ids(d: &Path) -> Vec<String> {
    let o = zloop(d, &["goal", "list", "--json"], None, &[]);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&o.out).unwrap_or_default();
    rows.iter().map(|r| r["id"].as_str().unwrap_or_default().to_string()).collect()
}

/// F9：`goal rm` 靠"目标文字里包含这个片段"就能对上一个目标，然后直接搬走。
/// 精确 id 是用户说清楚了要动谁，免问；猜出来的（文字片段 / id 前缀）要先把对上的那个
/// 打出来、等一句 y。
#[test]
fn archiving_by_a_guessed_needle_asks_first_but_an_exact_id_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把冷启动降到 1 秒"], None, &[]);
    zloop(d, &["goal", "new", "让 keep-awake 支持外接显示器"], None, &[]);
    assert_eq!(goal_ids(d), vec!["keep-awake", "g1"]);

    // 没人接话（stdin 直接 EOF，比如 runner 用 /dev/null 起的）：不能默默当"不同意"退个
    // 光秃秃的非零码，要说清楚这一步要确认、以及怎么免问
    let o = zloop(d, &["goal", "rm", "冷启动"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.out.contains("将要归档") && o.out.contains("把冷启动降到 1 秒"), "问之前要先把对上的那个打出来: {}", o.out);
    assert!(o.out.contains("zloop goal rm g1 --yes"), "要给出免问的写法: {}", o.out);
    assert!(o.err.contains("要确认") && o.err.contains("--yes"), "{}", o.err);
    assert_eq!(goal_ids(d), vec!["keep-awake", "g1"], "没同意就一个文件都不该动");
    assert!(!d.join(".zloop/archive").exists(), "连 archive/ 目录都不该建出来");

    // 明确说不 / 直接回车：都算不同意，退非零但不是报错
    for answer in ["n\n", "\n", "别\n"] {
        let o = zloop(d, &["goal", "rm", "冷启动"], Some(answer), &[]);
        assert_eq!(o.code, 1, "答 {answer:?}: {}{}", o.out, o.err);
        assert!(o.out.contains("已取消"), "答 {answer:?}: {}", o.out);
        assert_eq!(goal_ids(d), vec!["keep-awake", "g1"], "答 {answer:?} 之后清单不该变");
    }

    // 当前目标不能归档：这一条要在**问之前**就拒掉
    let o = zloop(d, &["goal", "rm", "keep-awake 支持"], Some("y\n"), &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("是当前目标"), "{}", o.err);
    assert!(!o.out.contains("确认归档"), "不能先问完 y 再说其实不能归档: {}", o.out);

    // 答 y 才真搬
    let o = zloop(d, &["goal", "rm", "冷启动"], Some("y\n"), &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("已归档"), "{}", o.out);
    assert_eq!(goal_ids(d), vec!["keep-awake"]);

    // id 前缀也是猜的，同样要问
    zloop(d, &["goal", "new", "第二个目标"], None, &[]);
    zloop(d, &["goal", "switch", "keep-awake"], None, &[]);
    let o = zloop(d, &["goal", "rm", "g"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.out.contains("id 前缀"), "要说清是按哪一档对上的: {}", o.out);
    assert_eq!(goal_ids(d), vec!["keep-awake", "g1"]);

    // --yes 跳过（stdin 依然是空的，证明它根本没去读）
    let o = zloop(d, &["goal", "rm", "第二个", "--yes"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(!o.out.contains("确认归档"), "{}", o.out);
    assert_eq!(goal_ids(d), vec!["keep-awake"]);

    // 精确 id：现状不变，一句都不问
    zloop(d, &["goal", "new", "第三个目标"], None, &[]);
    zloop(d, &["goal", "switch", "keep-awake"], None, &[]);
    let ids = goal_ids(d);
    assert_eq!(ids.len(), 2, "{ids:?}");
    let parked = ids[1].clone();
    let o = zloop(d, &["goal", "rm", &parked], None, &[]);
    assert_eq!(o.code, 0, "精确 id 不该被新的确认挡住: {}{}", o.out, o.err);
    assert!(!o.out.contains("确认归档") && !o.out.contains("将要归档"), "{}", o.out);
    assert!(o.out.contains("已归档"), "{}", o.out);
    assert_eq!(goal_ids(d), vec!["keep-awake"]);
    assert_eq!(fs::read_dir(d.join(".zloop/archive")).unwrap().count(), 3, "三次都只是搬家");
}

/// 读不出来的目标不能被静默隐藏，也不能挡住"把坏的停到一边，开个干净的"这条路。
#[test]
fn a_broken_current_goal_can_be_parked_listed_and_archived() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "会被写坏的目标"], None, &[]);
    fs::write(state::state_path(d), "{\"version\":1,\"goal\":").unwrap();

    let o = zloop(d, &["goal", "new", "干净的新目标"], None, &[]);
    assert_eq!(o.code, 0, "损坏的当前目标也要能停走: {}{}", o.out, o.err);

    let o = zloop(d, &["goal", "list", "--json"], None, &[]);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&o.out).unwrap();
    assert_eq!(rows.len(), 2, "坏掉的那份要出现在清单里，不能静默消失: {}", o.out);
    let broken: Vec<&serde_json::Value> = rows.iter().filter(|r| r["status"] == "broken").collect();
    assert_eq!(broken.len(), 1, "{}", o.out);
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_ne!(ids[0], ids[1], "停走的那份和新目标不能撞同一个 id: {ids:?}");
    assert!(zloop(d, &["goal", "list"], None, &[]).out.contains("损坏"));

    // 坏的那行也要清得掉
    let broken_id = broken[0]["id"].as_str().unwrap().to_string();
    let o = zloop(d, &["goal", "rm", &broken_id], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(!zloop(d, &["goal", "list"], None, &[]).out.contains("损坏"));
}

/// 目标全停着（没有当前目标）时，项目仍然要能被找到、被恢复——包括从子目录。
#[test]
fn a_project_without_a_current_goal_is_still_found_from_a_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "目标一"], None, &[]);
    zloop(d, &["goal", "new", "目标二"], None, &[]);
    // 手工制造"当前目标不在了"：等价于一次被打断的搬家留下的现场
    let path = state::state_path(d);
    let id = state::load(&path).unwrap().goal.id;
    fs::rename(&path, d.join(".zloop/goals").join(format!("{id}.json"))).unwrap();

    let sub = d.join("sub/deeper");
    fs::create_dir_all(&sub).unwrap();
    let o = zloop(&sub, &["goal", "list"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("当前没有目标在开着"), "图例不能指着不存在的 ▸: {}", o.out);
    assert!(o.out.contains("目标一") && o.out.contains("目标二"), "{}", o.out);
    assert!(o.out.contains("停放"), "停着的目标不叫「进行中」: {}", o.out);
    assert!(o.out.contains("zloop goal switch <id>"), "要给出恢复指令: {}", o.out);

    // status 的报错也要指路，而不是建议 init 把目标埋掉
    let o = zloop(&sub, &["status"], None, &[]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("当前没有目标") && o.err.contains("goal switch"), "{}", o.err);
    assert!(!o.err.contains("zloop init"), "别建议 init: {}", o.err);

    // 从子目录切回去
    let o = zloop(&sub, &["goal", "switch", &id], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert_eq!(state::load(&state::state_path(d)).unwrap().goal.text, "目标二");
}

/// 同一条 todo 不能同时派给两个会话：两个 agent 改同一批文件是净损失。
#[test]
fn next_does_not_hand_the_same_todo_to_two_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "抢活"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 唯一的活"], None, &[]);
    let a = [("CLAUDE_CODE_SESSION_ID", "sess-A")];
    let b = [("CLAUDE_CODE_SESSION_ID", "sess-B")];

    assert!(zloop(d, &["next"], None, &a).out.contains("RUN"));
    let o = zloop(d, &["next"], None, &b);
    assert!(o.out.contains("held_by_other"), "别的会话不能抢: {}", o.out);
    assert!(o.out.contains("sess-A"), "要说清楚在谁手里: {}", o.out);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.in_progress.unwrap().session.as_deref(), Some("sess-A"), "持有者不能被顶掉");
    assert!(st.ticks.is_empty(), "被挡住的一轮不该记 tick");

    // 自己再问一次照旧放行
    assert!(zloop(d, &["next"], None, &a).out.contains("RUN"));
    // stale_after_min = 0 关掉这个保护
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.policy.stale_after_min = 0;
    state::save(&p, &mut st).unwrap();
    assert!(zloop(d, &["next"], None, &b).out.contains("RUN"), "保护可以关掉");
}

/// `--force` 换目标之后，在飞会话的写回不能落到新目标头上。
#[test]
fn done_refuses_to_write_back_into_another_goal() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "目标X 重构缓存"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] X的活"], None, &[]);
    let x_id = state::load(&state::state_path(d)).unwrap().goal.id;
    zloop(d, &["goal", "new", "目标Y 写文档"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] Y的活"], None, &[]);
    zloop(d, &["goal", "switch", &x_id], None, &[]);

    let a = [("CLAUDE_CODE_SESSION_ID", "sess-A")];
    assert!(zloop(d, &["next"], None, &a).out.contains("RUN"));
    // 另一个终端强行换目标：要当场说清后果
    let o = zloop(d, &["goal", "switch", "写文档", "--force"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("还在别的会话手里"), "--force 要提醒后果: {}", o.out);

    // A 毫不知情地写回 → 必须被拦下，目标Y 一个字都不能被写脏
    let o = zloop(d, &["done", "t1", "--note", "X 的成果", "--approach", "X 的思路"], None, &a);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("目标X 重构缓存") && o.err.contains("goal switch"), "{}", o.err);
    let y = state::load(&state::state_path(d)).unwrap();
    assert_eq!(y.goal.text, "目标Y 写文档");
    assert_eq!(y.todos[0].status, "open", "Y 的活不能被 X 的成果标成完成");
    assert!(y.ticks.is_empty(), "Y 的账本不能多出 X 的那一轮");

    // 按提示切回去，写回落在正确的目标上
    zloop(d, &["goal", "switch", &x_id], None, &[]);
    let o = zloop(d, &["done", "t1", "--note", "X 的成果", "--approach", "X 的思路"], None, &a);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let x = state::load(&state::state_path(d)).unwrap();
    assert_eq!((x.todos[0].status.as_str(), x.ticks.len()), ("done", 1));
    assert_eq!(x.todos[0].note, "X 的成果");
}

/// `.zloop/log/` 是项目级的，而每个目标的 todo id 都从 t1 起——列日志必须认 tick 的账本，
/// 否则会把别的目标的过程当成本目标的证据摆出来。
#[test]
fn log_lists_only_the_current_goals_rounds() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "目标A"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] A的活"], None, &[]);
    zloop(d, &["done", "t1", "--note", "A的成果", "--approach", "A的思路"], None, &[]);
    let a_log = state::load(&state::state_path(d)).unwrap().ticks[0].log.clone().unwrap();

    zloop(d, &["goal", "new", "目标B"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] B的活"], None, &[]);
    zloop(d, &["done", "t1", "--note", "B的成果", "--no-doc"], None, &[]);
    let b_log = state::load(&state::state_path(d)).unwrap().ticks[0].log.clone().unwrap();
    assert_ne!(a_log, b_log);
    // 两份都在同一个目录里
    assert!(d.join(".zloop").join(&a_log).is_file() && d.join(".zloop").join(&b_log).is_file());

    // 当前是目标B：只列 B 的，并如实说明藏了几份
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains(&b_log), "{}", o.out);
    assert!(!o.out.contains(&a_log), "别的目标那份不该列出来: {}", o.out);
    assert!(o.out.contains("另有 1 份"), "{}", o.out);
    // --todo 也要按账本过滤，而不是按文件名里的 -t1-
    let o = zloop(d, &["log", "--todo", "t1"], None, &[]);
    assert!(o.out.contains(&b_log) && !o.out.contains(&a_log), "{}", o.out);
    // B 那一轮是 --no-doc，该标 ⚠（文件名可能带 -2 后缀，判断不能靠 ends_with("-done.md")）
    assert!(o.out.contains('⚠'), "{}", o.out);

    // 切回目标A：反过来
    zloop(d, &["goal", "switch", "目标A"], None, &[]);
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains(&a_log) && !o.out.contains(&b_log), "{}", o.out);
    assert!(!o.out.contains('⚠'), "A 那一轮有实现思路: {}", o.out);

    // tick 被 compact 归档后日志变成无主文件：仍然列出（宁可多列，不要把自己的历史藏起来）
    zloop(d, &["compact", "--keep-days", "0"], None, &[]);
    let o = zloop(d, &["log"], None, &[]);
    assert!(o.out.contains(&a_log), "无主文件也要列: {}", o.out);
    assert!(!o.out.contains(&b_log), "{}", o.out);
}

// ---------- 反馈通道（GOALS-REVIEW.md 的 W1） ----------

/// 人说的话必须有自己的位置：agent 自述之外的另一路信号，下一轮先看到它。
#[test]
fn feedback_records_what_the_human_said() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "写个解析器"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 解析嵌套括号", "--add", "[P1] 补测试"], None, &[]);
    zloop(d, &["next"], None, &[]);
    zloop(d, &["done", "t1", "--note", "用正则实现了", "--approach", "正则最快"], None, &[]);

    let words = "正则不行，输入会有嵌套括号，换成手写状态机";
    let o = zloop(d, &["feedback", "t1", words], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains(words), "{}", o.out);
    assert!(o.out.contains("zloop edit t1 --status open"), "已完成的 todo 要给出重做的路: {}", o.out);

    let st = state::load(&state::state_path(d)).unwrap();
    let last = st.ticks.last().unwrap();
    assert_eq!((last.outcome.as_str(), last.note.as_str()), ("feedback", words));
    assert_eq!(last.round, 1, "反馈不推进轮次");
    assert_eq!(st.todos[0].status, "done", "反馈是信号，不改 todo 状态");
    assert!(st.in_progress.is_none(), "反馈不碰在飞状态");

    // context：单列一节，且不在「当前判断」里重复
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("## 用户对上一轮的反馈"), "{}", o.out);
    assert_eq!(o.out.matches(words).count(), 1, "只出现一次: {}", o.out);
    let (head, _) = o.out.split_once("## 下一条").unwrap();
    assert!(head.contains(words), "要排在「下一条」前面: {}", o.out);

    // doc：和 agent 自述并排在同一条时间线上
    let o = zloop(d, &["doc", "t1"], None, &[]);
    assert!(o.out.contains("#### 实现思路") && o.out.contains("### 用户反馈"), "{}", o.out);
    let (before, after) = o.out.split_once("### 用户反馈").unwrap();
    assert!(before.contains("正则最快") && after.contains(words), "反馈排在它回应的那一轮之后: {}", o.out);

    // status：人自己也看得见
    assert!(zloop(d, &["status"], None, &[]).out.contains("反馈"), "status 要提一句");

    // 下一轮干完活之后，这条反馈就不再堆在交接包里
    zloop(d, &["done", "t2", "--note", "换成状态机了", "--approach", "手写状态机"], None, &[]);
    let o = zloop(d, &["context"], None, &[]);
    assert!(!o.out.contains("## 用户对上一轮的反馈"), "已处理的反馈不再占版面: {}", o.out);

    // 报错路径：不认识的 todo、空话
    assert_eq!(zloop(d, &["feedback", "t9", "x"], None, &[]).code, 2);
    assert_eq!(zloop(d, &["feedback", "t1", "   "], None, &[]).code, 2);
}

/// A-9：`edit --blocked-by t1` 加在 t1 自己身上 = 这条活永远轮不到。
///
/// 修复前直接被接受，此后 `next` 一路 `blocked` + "隔一阵重试"，重试到天荒地老
/// （依赖要 status == done，而要 done 得先被派出去），doctor 也不吭声。
/// 这里只钉 `edit` 这道闸：拒了、说了为什么、而且**一个字都没写进去**。
#[test]
fn edit_refuses_to_make_a_todo_depend_on_itself() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "自锁"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 唯一的活"], None, &[]);

    let o = zloop(d, &["edit", "t1", "--blocked-by", "t1"], None, &[]);
    assert_eq!(o.code, 2, "自依赖该被拒: {}{}", o.out, o.err);
    assert!(o.err.contains("不能依赖自己"), "要说清为什么被拒: {}", o.err);

    // 拒了就不该留下半截状态：blocked_by 空着，循环照旧派得出活
    let st = state::load(&state::state_path(d)).unwrap();
    assert!(st.todos[0].blocked_by.is_empty(), "被拒的 edit 不该写进去: {:?}", st.todos[0].blocked_by);
    let o = zloop(d, &["next", "--peek", "--json"], None, &[]);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    assert_eq!((v["should_run"].as_bool(), v["reason"].as_str()), (Some(true), Some("ready")), "{}", o.out);

    // 挡的是"依赖自己"，不是 --blocked-by 本身：指别人照旧收下
    zloop(d, &["plan", "--add", "[P0] 另一条"], None, &[]);
    assert_eq!(zloop(d, &["edit", "t1", "--blocked-by", "t2"], None, &[]).code, 0);
}

/// 连续失败之后循环停下等人——人开口说话，就是它该等到的东西。
#[test]
fn feedback_breaks_the_fail_streak() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "难搞的活"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 难搞的活"], None, &[]);
    for i in 1..=3 {
        zloop(
            d,
            &["done", "t1", "--outcome", "fail", "--note", &format!("第{i}次失败"), "--pitfall", "同一条路走不通"],
            None,
            &[],
        );
    }
    let o = zloop(d, &["next"], None, &[]);
    assert!(o.out.contains("fail_streak"), "{}", o.out);

    zloop(d, &["feedback", "t1", "别再试那条路了，先把依赖升到 2.0"], None, &[]);
    let o = zloop(d, &["next"], None, &[]);
    assert!(o.out.contains("RUN"), "人给了新信息，循环该继续: {}", o.out);

    // 不吃配额：feedback 不在 COUNTED 里
    let st = state::load(&state::state_path(d)).unwrap();
    let counted = zloop::tick::window_ticks(&st, zloop::state::now()).len();
    assert_eq!(counted, 3, "只有 3 次 fail 算进窗口: {counted}");
}

/// 「会话」那行要指向**最近干活**的会话。`summarize` 按首次出现排序，
/// 所以 `.last()` / `.rev().find()` 都会挑错：先露过面又长期没动的那个会赢。
#[test]
fn the_session_line_points_at_whoever_worked_last() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "两个会话交替"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P0] b", "--add", "[P0] c"], None, &[]);
    let (a, b) = ("sess-aaa-first", "sess-bbb-later");
    zloop(d, &["done", "t1", "--note", "1", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", a)]);
    zloop(d, &["done", "t2", "--note", "2", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", b)]);
    zloop(d, &["done", "t3", "--note", "3", "--no-doc"], None, &[("CLAUDE_CODE_SESSION_ID", a)]);

    // 秒级时间戳可能撞在一起，手工拉开：A 早 → B 中 → A 晚
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    for (i, at) in ["2026-08-28T10:00:00+08:00", "2026-08-28T11:00:00+08:00", "2026-08-28T12:00:00+08:00"].iter().enumerate()
    {
        st.ticks[i].at = (*at).into();
    }
    state::save(&p, &mut st).unwrap();

    let o = zloop(d, &["status"], None, &[]);
    assert!(o.out.contains(a), "会话行要给最后干活的那个: {}", o.out);
    assert!(!o.out.contains(b), "别给先露面但早就没动的那个: {}", o.out);
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains(a) && !o.out.contains(b), "{}", o.out);
}

/// 失败要变成"学到"：`--outcome fail` 必须留下坑，交接包里能看到踩过哪些。
#[test]
fn a_failed_round_must_leave_a_pitfall_and_it_shows_up_in_context() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "难搞的目标"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 链接第三方库", "--add", "[P1] 压测"], None, &[]);

    // 光说"失败了"不算数
    let o = zloop(d, &["done", "t1", "--outcome", "fail", "--note", "链接失败"], None, &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(o.err.contains("--pitfall") && o.err.contains("policy.require_pitfall"), "{}", o.err);
    assert!(o.err.contains("--outcome fail"), "报错要给出能直接抄的重试命令: {}", o.err);
    assert!(state::load(&state::state_path(d)).unwrap().ticks.is_empty(), "被拒的写回不能留下 tick");

    // 带上坑就放行，并且坑进了账本（不用回头解析 Markdown）
    let pit = "sqlite3 要用 brew 那份，系统自带的缺符号；下次先 otool -L";
    let o = zloop(d, &["done", "t1", "--outcome", "fail", "--note", "M1 上链接失败", "--pitfall", pit], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.ticks[0].pitfalls, vec![pit.to_string()]);

    // 交接包里看得到，而且排在「下一条」前面
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("## 本目标失败过的地方"), "{}", o.out);
    assert!(o.out.contains(pit), "{}", o.out);
    let (head, _) = o.out.split_once("## 下一条").unwrap();
    assert!(head.contains(pit), "失败要排在「下一条」前面: {}", o.out);

    // block 也算"卡住过的地方"
    zloop(d, &["done", "t2", "--block", "压测跑 CI 还是本地？"], None, &[]);
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("压测跑 CI 还是本地？"), "{}", o.out);

    // --no-doc 和 policy 开关都能绕过
    zloop(d, &["edit", "t1", "--status", "open"], None, &[]);
    assert_eq!(zloop(d, &["done", "t1", "--outcome", "fail", "--note", "又失败", "--no-doc"], None, &[]).code, 0);
    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    st.policy.require_pitfall = false;
    st.todos[0].status = "open".into();
    state::save(&p, &mut st).unwrap();
    assert_eq!(zloop(d, &["done", "t1", "--outcome", "fail", "--note", "第三次"], None, &[]).code, 0);
}

/// `stats` 回答的是"跑得顺不顺"，而且每个数字都必须和账本对得上——
/// 它是 reflect 的输入，算错了后面全错。
#[test]
fn stats_counts_match_the_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "有返工有失败的目标"], None, &[]);
    zloop(
        d,
        &["plan", "--add", "[P0] 顺利的", "--add", "[P0] 反复改的", "--add", "[P1] 会失败的", "--add", "[P2] 要问人的"],
        None,
        &[],
    );
    zloop(d, &["done", "t1", "--note", "一遍过", "--approach", "直接写完"], None, &[]);
    zloop(d, &["done", "t2", "--outcome", "progress", "--note", "改了一半"], None, &[]);
    zloop(d, &["done", "t2", "--outcome", "progress", "--note", "又一半"], None, &[]);
    zloop(d, &["done", "t2", "--note", "好了", "--no-doc"], None, &[]);
    zloop(d, &["done", "t3", "--outcome", "fail", "--note", "编不过", "--pitfall", "工具链版本不对"], None, &[]);
    zloop(d, &["done", "t4", "--block", "用哪个数据库？"], None, &[]);
    zloop(d, &["feedback", "t2", "其实可以更简单"], None, &[]);

    // --json：逐个字段和 state.json 现推的数字对照
    let o = zloop(d, &["stats", "--json"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
    let st = state::load(&state::state_path(d)).unwrap();
    let count = |outcome: &str| st.ticks.iter().filter(|t| t.outcome == outcome).count();
    let counted = st.ticks.iter().filter(|t| zloop::tick::COUNTED.contains(&t.outcome.as_str())).count();
    assert_eq!(v["rounds"], counted);
    assert_eq!(v["rework"], count("progress") + count("fail"));
    assert_eq!(v["fails"], count("fail"));
    assert_eq!(v["blocks"], count("block"));
    assert_eq!(v["feedback"], count("feedback"));
    assert_eq!(v["undocumented"], st.ticks.iter().filter(|t| t.documented == Some(false)).count());
    assert_eq!(v["done"], st.todos.iter().filter(|t| t.status == "done").count());
    assert_eq!(v["rework_rate"], 0.6);
    assert_eq!(v["first_try"], 1, "只有 t1 是一轮做完没返工的");

    // 每条 todo 的轮次也要对上
    let per: Vec<(String, u64)> = v["todos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["id"].as_str().unwrap().to_string(), t["rounds"].as_u64().unwrap()))
        .collect();
    assert_eq!(per, vec![("t1".into(), 1), ("t2".into(), 3), ("t3".into(), 1), ("t4".into(), 0)]);

    // 人看的那一屏：一句话结论 + 一张表，别的命令不重复它
    let o = zloop(d, &["stats"], None, &[]);
    assert!(o.out.contains("返工 3（60%）") && o.out.contains("一次过 1/2 条"), "{}", o.out);
    assert!(o.out.contains("最费劲") && o.out.contains("t2 返工 2 次"), "{}", o.out);
    for h in ["步骤", "id", "这一步做什么", "轮次", "返工", "文档", "结果"] {
        assert!(o.out.contains(h), "表头缺 {h}: {}", o.out);
    }
    assert!(o.out.contains("一次过") && o.out.contains("等你回话"), "{}", o.out);
    let widths: Vec<usize> = o.out.lines().filter(|l| l.contains('│')).map(zloop::style::width).collect();
    assert!(widths.windows(2).all(|w| w[0] == w[1]), "表格各行宽度不齐: {widths:?}");

    // 还没跑过的目标不该假装有数据
    let fresh = tempfile::tempdir().unwrap();
    zloop(fresh.path(), &["init", "还没开始"], None, &[]);
    assert!(zloop(fresh.path(), &["stats"], None, &[]).out.contains("还没有跑过任何一轮"));
}

/// `zloop reflect`：把材料摆齐给模型，人点头之后才落地。
#[test]
fn reflect_gathers_the_material_and_only_lands_when_you_say_so() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "回看试试"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a", "--add", "[P1] b"], None, &[]);
    zloop(d, &["done", "t1", "--outcome", "fail", "--note", "编不过", "--pitfall", "工具链版本不对"], None, &[]);
    zloop(d, &["feedback", "t1", "先升工具链再说"], None, &[]);
    zloop(d, &["remember", "bench.sh 要在 release 模式下跑"], None, &[]);
    zloop(d, &["remember", "bench 脚本必须用 release 模式跑，debug 差 3 倍"], None, &[]);

    let o = zloop(d, &["reflect"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    for want in ["## 现有约定", "## 现有经验", "## 失败与卡住过的地方", "## 我当时怎么说 vs 你怎么回的", "## 你要做的"]
    {
        assert!(o.out.contains(want), "材料包缺 {want}: {}", o.out);
    }
    assert!(o.out.contains("工具链版本不对") && o.out.contains("先升工具链再说"), "{}", o.out);
    // 机械体检认出那两条其实是同一件事
    assert!(o.out.contains("像是同一件事"), "{}", o.out);
    // 经验行不该带 RFC3339 时间戳（模型抄回来会变成双时间戳）
    assert!(!o.out.contains("+08:00 bench"), "经验只给日期不给完整时间戳: {}", o.out);
    // 光看不写：NOTES 一个字没动
    assert_eq!(zloop(d, &["reflect"], None, &[]).out, o.out, "reflect 是只读的");

    // 人点头之后落地；模型抄回来的编号和短横线都要容忍
    let o = zloop(
        d,
        &["reflect", "--apply"],
        Some("1. bench 脚本必须用 release 模式跑，debug 差 3 倍\n- 不要碰 migrations/\n\n"),
        &[],
    );
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("经验 2 → 2 条") && o.out.contains("备份"), "{}", o.out);
    // 经验行保留时间戳（约定不需要——它不轮换，日期没有意义）
    let notes = fs::read_to_string(d.join(".zloop/NOTES.md")).unwrap();
    assert!(notes.contains("bench 脚本必须用 release") && notes.contains("不要碰 migrations/"), "{notes}");
    assert!(!notes.contains("bench.sh 要在"), "被合并掉的那条不该还在: {notes}");
    assert_eq!(zloop::notes::read(d).lessons.len(), 2, "没写小标题就全算经验");
    let backups: Vec<_> = fs::read_dir(d.join(".zloop"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("NOTES.md.bak-"))
        .collect();
    assert_eq!(backups.len(), 1, "改之前先备份");
    // 下一轮就带上新的
    assert!(zloop(d, &["context"], None, &[]).out.contains("不要碰 migrations/"));

    // 空 stdin 不当成"清空"
    let o = zloop(d, &["reflect", "--apply"], Some("  \n\n"), &[]);
    assert_eq!(o.code, 2, "{}{}", o.out, o.err);
    assert!(fs::read_to_string(d.join(".zloop/NOTES.md")).unwrap().contains("migrations"), "被拒的调用不能动文件");
}

/// 回看那一轮对三条 streak 透明：插一轮反思不等于"失败被解决了"。
#[test]
fn a_reflect_round_does_not_reset_the_fail_streak() {
    let mut st = zloop::state::default_state("g", "g");
    let items = zloop::todo::parse_plan("[P0] a", 0);
    zloop::todo::add(&mut st, &items, false);
    let who = zloop::session::HostSession { host: zloop::session::Host::Cli, session: None };
    for _ in 0..3 {
        zloop::tick::record(&mut st, "fail", Some("t1"), "boom", &who).unwrap();
    }
    assert_eq!(zloop::tick::fail_streak(&st), 3);
    zloop::tick::record(&mut st, "reflect", None, "看了一眼", &who).unwrap();
    assert_eq!(zloop::tick::fail_streak(&st), 3, "回看不是进展，不该清掉失败计数");
    assert_eq!(zloop::tick::current_round(&st.ticks), 0, "回看不推进轮次");
}

/// 回路的最后一段：学到的东西要能升格成「约定」——每轮都带、不被经验窗口挤掉。
///
/// 为什么不写进 SKILL.md：那个文件是**全局**的（`~/.claude/skills/zloop/`），
/// 把某个项目的规矩写进去会污染别的项目。约定必须是项目级的。
#[test]
fn a_lesson_can_be_promoted_to_a_rule_that_ships_every_round() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "两层经验"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    for i in 1..=7 {
        zloop(d, &["remember", &format!("第 {i} 条经验，随便写点什么凑数")], None, &[]);
    }
    zloop(d, &["remember", "done 之前一定要跑 cargo test"], None, &[]);

    // 整理前：8 条经验，窗口只带 5 条，交接包会如实说漏了几条
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("最近 5 条，另有 3 条更早的没带上"), "{}", o.out);
    assert!(!o.out.contains("本项目的约定"), "还没有约定就不占版面: {}", o.out);
    // 材料包点名哪些已经掉出窗口
    let o = zloop(d, &["reflect"], None, &[]);
    assert!(o.out.contains("现有约定") && o.out.contains("（窗口外，模型看不到）"), "{}", o.out);
    assert!(o.out.contains("升格") && o.out.contains("## 约定"), "要教模型怎么写回: {}", o.out);

    // 人点头：升格两条，经验只留两条
    let o = zloop(
        d,
        &["reflect", "--apply"],
        Some("## 约定\n- done 之前一定要跑 cargo test\n- 不要碰 migrations/\n## 经验\n1. 第 6 条经验\n- 第 7 条经验\n"),
        &[],
    );
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("约定 0 → 2 条 · 经验 8 → 2 条"), "{}", o.out);

    // 关键：再写 6 条经验把窗口挤满，约定照样每轮都在
    for i in 1..=6 {
        zloop(d, &["remember", &format!("后来又加的第 {i} 条")], None, &[]);
    }
    let o = zloop(d, &["context"], None, &[]);
    let (head, _) = o.out.split_once("## 当前判断").unwrap();
    assert!(head.contains("## 本项目的约定（每轮都要遵守）"), "约定要紧跟目标: {}", o.out);
    assert!(o.out.contains("- done 之前一定要跑 cargo test") && o.out.contains("- 不要碰 migrations/"), "{}", o.out);
    assert!(!o.out.contains("第 6 条经验"), "经验照旧轮换: {}", o.out);

    // 篇幅极紧时先丢经验，约定和「下一条」不丢
    let o = zloop(d, &["context", "--budget", "700"], None, &[]);
    assert!(o.out.contains("本项目的约定") && o.out.contains("## 下一条"), "{}", o.out);

    // 老格式（没有小标题的一串 -）照旧能读：全算经验
    fs::write(d.join(".zloop/NOTES.md"), "# zloop notes\n\n- 老格式的一条\n").unwrap();
    let n = zloop::notes::read(d);
    assert!(n.rules.is_empty() && n.lessons.len() == 1, "{n:?}");
}

/// Warp 的 improver 读的是「agent 建议了什么」和「人最后怎么回应」之**差**——
/// 分成两栏各列一遍是看不出差的，必须配对到同一轮上。
#[test]
fn reflect_pairs_what_i_said_with_what_you_replied() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "配对试试"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 写解析器", "--add", "[P0] 加缓存", "--add", "[P1] 没人管的一条"], None, &[]);
    zloop(d, &["done", "t1", "--note", "用正则实现了", "--approach", "正则最快，输入看着很规整"], None, &[]);
    zloop(d, &["feedback", "t1", "正则不行，输入会有嵌套括号，换成手写状态机"], None, &[]);
    zloop(d, &["done", "t2", "--note", "加了个 LRU", "--approach", "标准库 HashMap + 手写链表"], None, &[]);
    zloop(d, &["feedback", "t2", "别自己写链表，用 lru crate"], None, &[]);
    zloop(d, &["done", "t3", "--note", "顺手做完了", "--approach", "没什么可说的"], None, &[]);

    let o = zloop(d, &["reflect"], None, &[]);
    assert!(o.out.contains("我当时怎么说 vs 你怎么回的"), "{}", o.out);
    // 成对出现：我的一句话结果 + 实现思路摘要，紧跟着人的原话
    let (_, tail) = o.out.split_once("### t1").unwrap();
    let block = tail.split("\n\n").next().unwrap();
    assert!(block.contains("用正则实现了") && block.contains("实现思路：正则最快"), "配对块要带上实现思路: {block}");
    assert!(block.contains("正则不行，输入会有嵌套括号"), "紧跟着人的原话: {block}");
    // 最近的排在前面
    assert!(o.out.find("### t2").unwrap() < o.out.find("### t1").unwrap(), "最近的在前: {}", o.out);
    // 没人回过话的轮次不占版面
    assert!(!o.out.contains("### t3"), "t3 没人回过话，不该出现: {}", o.out);
    assert!(!o.out.contains("顺手做完了"), "{}", o.out);
    // 缩进没有从源码里漏出来
    assert!(o.out.contains("\n_只列有人回过话的轮次"), "说明行不该带缩进: {:?}", o.out);

    // 配不上的反馈（这条 todo 还没写回过任何一轮）也要照实说
    zloop(d, &["plan", "--add", "[P2] 还没开始的一条"], None, &[]);
    zloop(d, &["feedback", "t4", "这条先别做"], None, &[]);
    let o = zloop(d, &["reflect"], None, &[]);
    let (_, tail) = o.out.split_once("### t4").unwrap();
    assert!(tail.contains("这条反馈之前没有已写回的轮次"), "{}", o.out);
}

/// 第三项机械体检：约定这一层也得有人管。
///
/// 经验有窗口兜底（写多了老的自己滚出去），约定**不轮换**——写多少条就每轮全量占多少篇幅，
/// 挤掉的是交接包尾部那些会被裁掉的节。所以条数超过阈值时得提一句，阈值本身要能调。
#[test]
fn reflect_flags_too_many_rules_and_the_threshold_is_tunable() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "约定攒多了"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    for i in 1..=10 {
        zloop(d, &["remember", "--rule", &format!("第 {i} 条约定，随便写点什么凑够长度")], None, &[]);
    }

    // 正好卡在默认阈值（10）上：不出声
    let o = zloop(d, &["reflect"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(!o.out.contains("条约定，超过"), "没超阈值就一声不吭: {}", o.out);
    // 阈值可调：调低就该出声，即使条数没变
    let o = zloop(d, &["reflect", "--max-rules", "8"], None, &[]);
    assert!(o.out.contains("## 机械体检"), "{}", o.out);
    assert!(o.out.contains("共 10 条约定，超过 8 条"), "{}", o.out);

    // 再加一条就越过默认阈值，不给 flag 也会提
    zloop(d, &["remember", "--rule", "第 11 条约定，随便写点什么凑够长度"], None, &[]);
    let o = zloop(d, &["reflect"], None, &[]);
    assert!(o.out.contains("共 11 条约定，超过 10 条"), "{}", o.out);
    // 提示要说清代价：约定不轮换、每轮全量进交接包，占掉多少篇幅
    assert!(o.out.contains("每轮全量进交接包"), "{}", o.out);
    let hit = o.out.lines().find(|l| l.contains("条约定，超过")).unwrap().to_string();
    assert!(hit.contains("约 233 字，占默认预算 5%"), "篇幅要算成真数字: {hit}");
    // 阈值往上调也认：调到 11 就不该再提
    let o = zloop(d, &["reflect", "--max-rules", "11"], None, &[]);
    assert!(!o.out.contains("条约定，超过"), "阈值调高就不该再提: {}", o.out);
    // 「你要做的」里那句劝也跟着阈值走，不再写死"十来条"
    assert!(o.out.contains("超过 11 条就该反省"), "{}", o.out);

    // 体检是只读的：提了这一句也不该动 NOTES
    assert_eq!(zloop::notes::read(d).rules.len(), 11);

    // 经验那两项体检不受影响：约定超标的同时照样认出重复的经验
    zloop(d, &["remember", "bench.sh 要在 release 模式下跑"], None, &[]);
    zloop(d, &["remember", "bench 脚本必须用 release 模式跑，debug 差 3 倍"], None, &[]);
    let o = zloop(d, &["reflect"], None, &[]);
    assert!(o.out.contains("共 11 条约定，超过 10 条") && o.out.contains("像是同一件事"), "三项体检互不干扰: {}", o.out);
}

/// `remember --rule`：不绕一整轮 reflect 也能顺手钉一条约定。
#[test]
fn remember_rule_pins_a_convention_without_a_reflect_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "直接钉约定"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a"], None, &[]);
    zloop(d, &["remember", "普通经验一条"], None, &[]);
    let stamped = fs::read_to_string(d.join(".zloop/NOTES.md")).unwrap();
    let stamp = stamped.lines().find(|l| l.contains("普通经验一条")).unwrap().to_string();

    let o = zloop(d, &["remember", "--rule", "done 之前一定要跑 cargo test"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("约定 +1（共 1 条"), "{}", o.out);
    zloop(d, &["remember", "--rule", "不要碰 migrations/"], None, &[]);

    // 同一条不会钉两遍
    let o = zloop(d, &["remember", "--rule", "done 之前一定要跑 cargo test"], None, &[]);
    assert!(o.out.contains("已经在了（共 2 条）"), "{}", o.out);

    let n = zloop::notes::read(d);
    assert_eq!(n.rules.len(), 2);
    assert_eq!(n.lessons.len(), 1, "经验没被动过");
    // 加约定是纯增量，不该像 reflect --apply 那样每次留一份备份
    let baks = fs::read_dir(d.join(".zloop"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
        .count();
    assert_eq!(baks, 0, "增量操作不备份");
    // 重写文件不能把经验的原始时刻抹掉
    assert!(fs::read_to_string(d.join(".zloop/NOTES.md")).unwrap().contains(&stamp), "时间戳要原样保留");

    // 立刻生效：每轮都带
    let o = zloop(d, &["context"], None, &[]);
    assert!(o.out.contains("## 本项目的约定（每轮都要遵守）") && o.out.contains("- 不要碰 migrations/"), "{}", o.out);
    // 空话不收
    assert_eq!(zloop(d, &["remember", "--rule", "   "], None, &[]).code, 2);
}

/// 人写的正文里出现 `--xxx` 是常事（尤其在记「哪个 flag 不该用」这种坑的时候）。
/// 装散文的参数都要 `allow_hyphen_values`，否则整条命令被 clap 拒掉——写 t8 的
/// `--decision` 时实撞过一次。
#[test]
fn prose_arguments_accept_text_that_starts_with_a_dash() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "--force 开头的目标名"], None, &[]);
    assert_eq!(state::load(&state::state_path(d)).unwrap().goal.text, "--force 开头的目标名");
    zloop(d, &["plan", "--add", "[P0] 一条活", "--add", "[P1] --force 别用"], None, &[]);

    let o = zloop(
        d,
        &[
            "done",
            "t1",
            "--note",
            "--rule 只给人用",
            "--approach",
            "--force 会归档旧目标",
            "--decision",
            "--apply 那条路径会删东西",
            "--pitfall",
            "-x 开头的一句话",
        ],
        None,
        &[],
    );
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    let t = st.ticks.last().unwrap();
    assert_eq!(t.note, "--rule 只给人用");
    assert_eq!(t.pitfalls, vec!["-x 开头的一句话".to_string()]);
    assert_eq!(st.todos[1].text, "[P1] --force 别用".trim_start_matches("[P1] "));

    // 其余入口同样
    assert_eq!(zloop(d, &["remember", "--rule", "--apply 之前先看备份"], None, &[]).code, 0);
    assert_eq!(zloop(d, &["feedback", "t2", "--no-doc 用多了就没文档了"], None, &[]).code, 0);
    assert_eq!(zloop(d, &["goal", "new", "--reflect-every 相关的活"], None, &[]).code, 0);
    assert_eq!(zloop::notes::read(d).rules, vec!["--apply 之前先看备份".to_string()]);

    // 代价是有界的：打错的 flag 值照旧报错，漏写值也不会被悄悄吞掉
    zloop(d, &["goal", "switch", "--force 开头的目标名"], None, &[]);
    assert_ne!(zloop(d, &["done", "t1", "--outcome", "faill"], None, &[]).code, 0);
    assert_ne!(zloop(d, &["done", "t1", "--note", "--approach", "真正的思路"], None, &[]).code, 0);
}

/// 做完一条就重估：**沉默是默认**，只有账本里读得出偏离信号才提一句。
/// 依据见 docs/ADAPTIVE-REPLAN.md §2——每轮都催重规划会制造计划抖动。
#[test]
fn done_only_nudges_a_replan_when_the_ledger_says_something_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把冷启动降到 1 秒"], None, &[]);
    zloop(
        d,
        &["plan", "--add", "[P0] 找最慢的三处", "--add", "[P0] 加缓存", "--add", "[P1] 补基准 :: bench.sh 跑得出数"],
        None,
        &[],
    );

    // 一切顺利：一个字都不多说
    let o = zloop(d, &["done", "t1", "--note", "定位到 3 处", "--approach", "tracing 打点"], None, &[]);
    assert!(!o.out.contains("计划可能要调整"), "顺利的一轮不该打扰: {}", o.out);

    // 连续两轮没做完 → 停滞信号
    zloop(d, &["done", "t2", "--outcome", "progress", "--note", "改了一半"], None, &[]);
    let o = zloop(d, &["done", "t2", "--outcome", "progress", "--note", "又一半"], None, &[]);
    assert!(o.out.contains("计划可能要调整") && o.out.contains("t2 连续 2 轮没做完"), "{}", o.out);
    assert!(o.out.contains("zloop replan"), "要给出下一步: {}", o.out);

    // 人开口之后，信号不能消失——那正是最该重估的时刻
    zloop(d, &["feedback", "t2", "缓存方向不对，先量再说"], None, &[]);
    let o = zloop(d, &["done", "t2", "--outcome", "progress", "--note", "第三次"], None, &[]);
    assert!(o.out.contains("t2 有你的反馈"), "人说过的话要一直算数: {}", o.out);
    assert!(o.out.contains("连续 3 轮没做完"), "「在拖」不会因为人说了句话就不拖: {}", o.out);

    // 材料包：目标 + 剩下的 + 信号 + 人的原话 + 只提最小改动
    let o = zloop(d, &["replan"], None, &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("# 重估一次：把冷启动降到 1 秒"), "{}", o.out);
    assert!(o.out.contains("## 剩下的 2 条") && o.out.contains("bench.sh 跑得出数"), "剩余任务连验收标准一起给: {}", o.out);
    assert!(o.out.contains("[stalled]") && o.out.contains("[rework]") && o.out.contains("[feedback]"), "{}", o.out);
    assert!(o.out.contains("缓存方向不对，先量再说"), "光说「有反馈」没用，要给原话: {}", o.out);
    assert!(o.out.contains("别重开一张清单") && o.out.contains("人点头之后"), "{}", o.out);
    assert!(o.out.contains("不改是完全合格的结论"), "别为了改而改: {}", o.out);
    // 只读：跑两次一样，什么都没动
    let before = fs::read_to_string(state::state_path(d)).unwrap();
    assert_eq!(zloop(d, &["replan"], None, &[]).out, o.out);
    assert_eq!(fs::read_to_string(state::state_path(d)).unwrap(), before, "replan 是只读的");

    // 全做完之后不再提（没有「后续」可调整了）
    zloop(d, &["done", "t2", "--note", "好了", "--no-doc"], None, &[]);
    let o = zloop(d, &["done", "t3", "--note", "好了", "--no-doc"], None, &[]);
    assert!(!o.out.contains("计划可能要调整"), "{}", o.out);
}

#[test]
fn replan_apply_swaps_the_route_without_touching_history() {
    // 用户要的那一幕：5 条 todo，做到第 2 条发现整条路线的前提没了，
    // 于是照新现状重排，做过的一条不丢。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把冷启动降到 1 秒"], None, &[]);
    zloop(
        d,
        &[
            "plan",
            "--add",
            "[P0] 量最慢三处 :: 有数",
            "--add",
            "[P0] 加缓存 :: 快 500ms",
            "--add",
            "[P0] 复测 :: 基准过",
            "--add",
            "[P1] 补基准 :: bench 跑得出",
            "--add",
            "[P1] 写文档 :: README 有一节",
        ],
        None,
        &[],
    );
    zloop(d, &["done", "t1", "--note", "慢在同步读配置", "--approach", "打点", "--no-doc"], None, &[]);
    zloop(
        d,
        &[
            "done",
            "t2",
            "--note",
            "只省 30ms",
            "--approach",
            "LRU",
            "--no-doc",
            "--rethink",
            "瓶颈在反序列化，后三条前提没了",
        ],
        None,
        &[],
    );

    let plan = "[P0] 量反序列化耗时 :: 有逐字段耗时表
                [P0] 换零拷贝路径 :: 快 300ms 以上
                [P0] 惰性加载大配置 :: 只解析用得到的
                [P0] 复测冷启动 :: 端到端 1 秒内
                [P1] 补基准 :: bench.sh 跑得出数
                [P1] 写文档 :: README 记下为什么弃用缓存
";
    let o = zloop(d, &["replan", "--apply", "--why", "实测瓶颈在反序列化，加缓存整条路线作废"], Some(plan), &[]);
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("换掉 3 条、新排 6 条、保留 2 条"), "{}", o.out);
    assert!(o.out.contains("旧账本备份在"), "改前必须备份: {}", o.out);

    let st = state::load(&state::state_path(d)).unwrap();
    // 历史一条不丢，而且 id 不复用
    let done: Vec<&str> = st.todos.iter().filter(|t| t.status == "done").map(|t| t.id.as_str()).collect();
    assert_eq!(done, vec!["t1", "t2"], "做过的原样留着");
    assert_eq!(st.goal.text, "把冷启动降到 1 秒", "目标文字不许被重排改掉");
    let open: Vec<&str> = st.todos.iter().filter(|t| t.status == "open").map(|t| t.id.as_str()).collect();
    assert_eq!(open, vec!["t6", "t7", "t8", "t9", "t10", "t11"], "新 id 从 next_id 往后发，不复用 t3-t5");
    assert_eq!(st.ticks.iter().filter(|t| t.outcome == "done").count(), 2, "老 tick 一条不丢");
    assert!(st.ticks.iter().any(|t| t.outcome == "replan" && t.note.contains("换掉 3 条")), "改了什么要记进账本");
    assert!(st.ticks.iter().any(|t| t.rethink.is_some()), "那句 rethink 是历史，留着");
    // 重排之后信号消了（边沿）
    assert!(!zloop(d, &["replan"], None, &[]).out.contains("[rethink]"), "重排过就不该再响");
}

#[test]
fn replan_apply_refuses_rather_than_half_changing_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a :: 验a", "--add", "[P0] b :: 验b", "--add", "[P0] c :: 验c"], None, &[]);
    // 跨过一秒再拒绝：`updated_at` 是秒精度，同一秒内多写一次时间戳不变，
    // 「拒绝了却还是落了一次盘」这种 bug 会假绿溜过去（踩过——这条测试当年时红时绿，
    // 红的那几次才是对的）。
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let b = fs::read_to_string(state::state_path(d)).unwrap();
    // 快照当参数传，别让闭包捕获——`before` 后面会被 shadow，闭包捕获的还是定义时那个（踩过）
    let refused = |o: &Out, guard: &str, before: &str| {
        assert_eq!(o.code, 2, "该拒绝: {}{}", o.out, o.err);
        assert!(o.err.contains(guard), "要指名是哪条护栏（期望「{guard}」）: {}", o.err);
        assert_eq!(fs::read_to_string(state::state_path(d)).unwrap(), before, "拒绝了就一个字都不能动");
    };
    refused(&zloop(d, &["replan", "--apply", "--why", "收工"], Some(""), &[]), "清单不能空", &b);
    refused(&zloop(d, &["replan", "--apply", "--why", "换路"], Some("[P0] 没验收标准的一条\n"), &[]), "每条都要可验证", &b);
    refused(&zloop(d, &["replan", "--apply", "--why", ""], Some("[P0] a :: b\n"), &[]), "说清为什么", &b);
    let many: String = (1..=20).map(|i| format!("[P0] 第{i}条 :: 验{i}\n")).collect();
    refused(&zloop(d, &["replan", "--apply", "--why", "炸开"], Some(&many), &[]), "规模上限", &b);

    // 有轮次在飞：那个 agent 手上拿的 todo 可能正要被换掉
    zloop(d, &["next", "--json"], None, &[("CLAUDE_CODE_SESSION_ID", "A")]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let b = fs::read_to_string(state::state_path(d)).unwrap();
    refused(&zloop(d, &["replan", "--apply", "--why", "抢改"], Some("[P0] x :: y\n"), &[]), "不动在飞的轮次", &b);
}

#[test]
fn replan_apply_never_deletes_a_question_that_is_waiting_on_you() {
    // 等人回话的 todo 身上挂着一个**给人的问题**，agent 没资格替人把问题删掉。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "g"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] a :: 验a", "--add", "[P0] b :: 验b"], None, &[]);
    zloop(d, &["done", "t1", "--outcome", "progress", "--block", "用 A 方案还是 B 方案？", "--note", "等人"], None, &[]);

    let o = zloop(
        d,
        &["replan", "--apply", "--why", "换个路线"],
        Some(
            "[P0] 新路线 :: 验新
",
        ),
        &[],
    );
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    let st = state::load(&state::state_path(d)).unwrap();
    let t1 = st.todos.iter().find(|t| t.id == "t1").unwrap();
    assert!(t1.blocked_by.contains(&"user".to_string()), "等人回话的那条要原样留着: {:?}", t1);
    assert!(st.todos.iter().any(|t| t.id == "t3"), "新的排上了");
    assert!(!st.todos.iter().any(|t| t.id == "t2"), "普通的 open 该被换掉");
}

#[test]
fn an_unreadable_notes_file_is_never_silently_overwritten() {
    // 读文件失败降级成 Default::default() 只有在**纯读**路径上才安全。`add_rule` 是读-改-写：
    // 那次降级会被写回磁盘，把用户攒下的全部约定和经验**永久删掉**，而且一声不吭。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "别弄丢我的经验"], None, &[]);
    zloop(d, &["remember", "--rule", "done 之前先跑测试"], None, &[]);
    zloop(d, &["remember", "bench.sh 要在 release 下跑"], None, &[]);
    let before = fs::read(d.join(".zloop/NOTES.md")).unwrap();
    assert!(zloop(d, &["context"], None, &[]).out.contains("done 之前先跑测试"));

    // 塞一个非 UTF-8 字节（磁盘坏块、别的工具用了 GBK、传输截断都会造成）
    let mut broken = before.clone();
    broken.extend_from_slice(&[0xff, 0xfe]);
    fs::write(d.join(".zloop/NOTES.md"), &broken).unwrap();

    let o = zloop(d, &["remember", "--rule", "又一条约定"], None, &[]);
    assert_ne!(o.code, 0, "读不出来就不该假装读到了空的: {}{}", o.out, o.err);
    assert!(o.err.contains("NOTES.md") && o.err.contains("读不"), "要说清是哪个文件读不了: {}", o.err);

    let after = fs::read(d.join(".zloop/NOTES.md")).unwrap();
    assert_eq!(after, broken, "拒绝之后一个字节都不能动——原件还得留着让人抢救");
    // 把坏字节去掉，老内容必须还在
    fs::write(d.join(".zloop/NOTES.md"), &before).unwrap();
    let n = zloop::notes::read(d);
    assert_eq!(n.rules.len(), 1, "约定还在: {:?}", n.rules);
    assert_eq!(n.lessons.len(), 1, "经验还在: {:?}", n.lessons);
}

#[test]
fn context_says_out_loud_that_this_round_has_no_project_rules() {
    // 护栏丢失只在**丢失的那一轮**有意义。`zloop doctor` 会报 `unreadable_notes`，可 doctor
    // 只在有人敲的时候才说话——无头 runner 一轮都不会跑它。每轮必跑的是 `zloop context`，
    // 所以这个声得由它自己来出。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "别让我在没有约定的情况下开工"], None, &[]);
    zloop(d, &["remember", "--rule", "done 之前先跑测试"], None, &[]);
    zloop(d, &["remember", "bench.sh 要在 release 下跑"], None, &[]);

    // 读得出来的时候一个字都不许多说：这行只在真出事时出现，否则就成了每轮都刷的噪音
    let ok = zloop(d, &["context"], None, &[]);
    assert!(ok.out.contains("done 之前先跑测试"), "{}", ok.out);
    assert!(!ok.err.contains("NOTES.md"), "正常情况下不该有告警: {:?}", ok.err);

    // NOTES.md 还在，但混进了非 UTF-8 字节
    let mut broken = fs::read(d.join(".zloop/NOTES.md")).unwrap();
    broken.extend_from_slice(&[0xff, 0xfe]);
    fs::write(d.join(".zloop/NOTES.md"), &broken).unwrap();

    let o = zloop(d, &["context"], None, &[]);
    // 先把"静默"这件事本身钉住：包里确实少了这两整节
    assert!(!o.out.contains("done 之前先跑测试"), "约定确实没了（这正是要喊的原因）: {}", o.out);
    assert!(!o.out.contains("bench.sh"), "经验也没了: {}", o.out);
    // 这一轮就得听得见，而且要说清是哪个文件、丢了什么
    assert!(o.err.contains("NOTES.md"), "得点名是哪个文件: {:?}", o.err);
    assert!(o.err.contains("约定") && o.err.contains("经验"), "得说清少了哪两节: {:?}", o.err);
    assert_eq!(o.err.lines().filter(|l| l.contains("NOTES.md")).count(), 1, "一行就够: {:?}", o.err);
    // 交接包本身照旧交付、exit 0：没有约定是能降级干活的，把整轮劝退比少两节更糟
    assert_eq!(o.code, 0, "{}{}", o.out, o.err);
    assert!(o.out.contains("## 目标") && o.out.contains("## 下一条"), "能读到的那部分照旧给: {}", o.out);

    // 文件根本不存在 ≠ 读不出来：没记过东西的项目不该每轮被喊一次
    fs::remove_file(d.join(".zloop/NOTES.md")).unwrap();
    let o = zloop(d, &["context"], None, &[]);
    assert!(!o.err.contains("NOTES.md"), "没这个文件是合法状态，别喊: {:?}", o.err);
}

#[test]
fn a_successful_round_can_still_say_the_rest_of_the_plan_is_dead() {
    // 最该重规划的那种场景**不偏离**：那一轮顺利完成，可它的结论把剩下几条的前提推翻了。
    // 五个偏离信号（feedback/stalled/fail_streak/rework/blocked）一个都不会响。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "把冷启动降到 1 秒"], None, &[]);
    zloop(
        d,
        &[
            "plan",
            "--add",
            "[P0] 量出最慢的三处",
            "--add",
            "[P0] 给最慢那处加缓存",
            "--add",
            "[P0] 复测",
            "--add",
            "[P1] 补基准",
            "--add",
            "[P1] 写文档",
        ],
        None,
        &[],
    );
    zloop(d, &["done", "t1", "--note", "慢在同步读 3 个大配置", "--approach", "打点", "--no-doc"], None, &[]);

    // 顺利做完第二条，把结论写进 note 里——zloop 读不出来，照旧沉默
    let o =
        zloop(d, &["done", "t2", "--note", "加了缓存只省 30ms，瓶颈在反序列化", "--approach", "LRU", "--no-doc"], None, &[]);
    assert!(!o.out.contains("计划可能要调整"), "光写在 note 里 zloop 认不出来，不该假装认得出: {}", o.out);
    assert!(!zloop(d, &["replan"], None, &[]).out.contains("[rethink]"), "没说出口就没有信号");

    // 说出口：一句 --rethink 就够
    let o = zloop(
        d,
        &[
            "done",
            "t3",
            "--note",
            "复测确认",
            "--approach",
            "基准",
            "--no-doc",
            "--rethink",
            "瓶颈在反序列化，t4/t5 全建立在「缓存有效」这个前提上，前提没了",
        ],
        None,
        &[],
    );
    assert!(o.out.contains("计划可能要调整") && o.out.contains("后续走不通"), "{}", o.out);
    let packet = zloop(d, &["replan"], None, &[]).out;
    assert!(packet.contains("[rethink] t3 那一轮说后续走不通"), "{packet}");
    assert!(packet.contains("干活的人说后续走不通（原话）"), "光说「有人说走不通」没用，要给原话: {packet}");
    assert!(packet.contains("前提没了"), "原话要完整给出: {packet}");
    assert!(packet.contains("可能全都成功了"), "要点明「走不通的不是那一轮」: {packet}");
    assert!(packet.contains("别重开一张清单"), "默认仍然是 plan repair: {packet}");
    assert!(packet.contains("那就照新的现状重排"), "但前提被推翻时要允许重排，不能硬凑最小改动: {packet}");

    // 账本里留得住：不是只在提示里一闪而过
    let st = state::load(&state::state_path(d)).unwrap();
    let t = st.ticks.iter().find(|t| t.todo.as_deref() == Some("t3")).unwrap();
    assert!(t.rethink.as_deref().is_some_and(|r| r.contains("前提没了")), "{:?}", t.rethink);

    // 边沿不是锁存：真的重估过之后，同一句话不该再触发（`blocked` 当年就是栽在这——
    // 一条挂着的 todo 让 4 小时长跑里 5 次重估全由同一个信号触发）。
    // `zloop replan` 是只读的、不记 tick，所以这里手工塞一条 replan tick 模拟「重估过了」。
    let o = zloop(d, &["done", "t4", "--note", "补了基准", "--approach", "x", "--no-doc"], None, &[]);
    assert!(o.out.contains("后续走不通"), "还没重估过，信号该一直在: {}", o.out);

    let p = state::state_path(d);
    let mut st = state::load(&p).unwrap();
    let mut replan_tick = st.ticks.last().unwrap().clone();
    replan_tick.outcome = "replan".into();
    replan_tick.todo = None;
    replan_tick.rethink = None;
    st.ticks.push(replan_tick);
    state::save(&p, &mut st).unwrap();

    let o = zloop(d, &["done", "t5", "--note", "写完了", "--approach", "x", "--no-doc"], None, &[]);
    assert!(!o.out.contains("后续走不通"), "重估过一次就该消停，别变成锁存: {}", o.out);
    assert!(!zloop(d, &["replan"], None, &[]).out.contains("[rethink]"), "材料包里同理");
    let st = state::load(&p).unwrap();
    assert!(st.ticks.iter().any(|t| t.rethink.is_some()), "但账本里那句话要留着，是历史不是状态");
}

#[test]
fn a_finished_todo_no_longer_counts_as_waiting_on_you() {
    // `blocked_by` 是履历不是现状：todo 做完之后这一栏原样留着，用来记「这条当初卡过人」。
    // 信号要是不排除终态，一条早就 done 的 todo 会让「在等你回话」永远响下去。
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    zloop(d, &["init", "升级本机工具链"], None, &[]);
    zloop(d, &["plan", "--add", "[P0] 换二进制", "--add", "[P1] 收尾验收"], None, &[]);

    // t1 卡在人身上：该响
    let o =
        zloop(d, &["done", "t1", "--outcome", "progress", "--block", "现在装还是跑完再装？", "--note", "等人"], None, &[]);
    assert!(o.out.contains("t1 在等你回话"), "挂起时要提醒: {}", o.out);
    assert!(zloop(d, &["replan"], None, &[]).out.contains("[blocked]"), "材料包里也要有");

    // 人回话、t1 做完了：blocked_by 还留着 user，但它不再等任何人
    let st = state::load(&state::state_path(d)).unwrap();
    assert!(st.todos[0].blocked_by.contains(&"user".to_string()), "履历要留着");
    zloop(d, &["done", "t1", "--note", "装好了", "--approach", "cargo install", "--no-doc"], None, &[]);
    let st = state::load(&state::state_path(d)).unwrap();
    assert_eq!(st.todos[0].status, "done");
    assert!(st.todos[0].blocked_by.contains(&"user".to_string()), "done 之后 blocked_by 原样保留（这正是坑的来源）");

    let o = zloop(d, &["done", "t2", "--outcome", "progress", "--note", "在做"], None, &[]);
    assert!(!o.out.contains("t1 在等你回话"), "t1 已经做完了，别再说它在等人: {}", o.out);
    assert!(!zloop(d, &["replan"], None, &[]).out.contains("t1 在等你回话"), "材料包同理");
}

/// A-8：时间参数「装得下 i64」就 panic，装不下反而有好错误提示。
///
/// `--since 99999999999999999999d` 这种 i64 都装不下的，`digits.parse::<i64>()` 失败，
/// 落到写好的那条友好错误上（exit 2 + 「用 2h / 30m / 7d」）；而刚好装得下的
/// `99999999999d` 走进 `now() - Duration::days(n)`，直接 panic 退 101。
/// 作者想到了"这串东西可能不是数字"，没想到"是数字但算不出来"——两者是同一类输入错误，
/// 就该给同一种交代。修法是 `try_*` + `checked_sub_signed`，越界的落回同一条路径。
#[test]
fn out_of_range_time_arguments_get_the_same_friendly_error_as_garbage() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(zloop(d, &["init", "alpha"], None, &[]).code, 0);
    zloop(d, &["plan"], Some("[P1] one\n"), &[]);

    // 撤掉修复时下面每一条都是 exit 101 + 一行 Rust panic
    for arg in ["99999999999d", "99999999999h", "9223372036854775807d"] {
        for flag in ["--since", "--until"] {
            let o = zloop(d, &["doc", "--all", flag, arg], None, &[]);
            assert_eq!(o.code, 2, "doc {flag} {arg} 该走输入错误那条路：{}{}", o.out, o.err);
            assert!(o.err.contains("看不懂的时间"), "得给和乱码同一条提示：{}", o.err);
            assert!(!o.err.contains("panicked"), "{}", o.err);
        }
    }
    // compact 是另一个入口、同一个根因：`now() - Duration::days(keep_days)`
    for arg in ["99999999999", "999999999999999", "9223372036854775807"] {
        let o = zloop(d, &["compact", "--keep-days", arg, "--force"], None, &[]);
        assert_eq!(o.code, 2, "compact --keep-days {arg}：{}{}", o.out, o.err);
        assert!(o.err.contains("算不出截止时间"), "得说清是这个数太大：{}", o.err);
        assert!(!o.err.contains("panicked"), "{}", o.err);
    }
    // 正常取值一条都不能被误伤
    assert_eq!(zloop(d, &["doc", "--all", "--since", "7d"], None, &[]).code, 0);
    assert_eq!(zloop(d, &["compact", "--keep-days", "30", "--force"], None, &[]).code, 0);
}

/// A-7 的 CLI 面：`policy.window_hours` 越界时，**每轮都要走的那三条命令**不能崩。
///
/// 炸的正好是 skill 每轮的 `context` → `next` 和 runner 每轮的 decide；人拿到的是一行
/// Rust panic 加一句 "run with RUST_BACKTRACE=1"，整个项目目录就此敲不动。
/// 单元测试（tick_test）钉的是钳位本身，这里钉的是"用户真敲的那几条命令还能用"。
///
/// 越界的 `window_hours` 有**两处**要钳，走的是两条不同的分支（t28）：`window_span`
/// 每次 decide 都过，配额没满时也走；而 `throttled` 那一支的等待封顶（`window_hours * 60`）
/// 只有**配额占满**时才走到——`max_runs` 没满的项目怎么敲都碰不到它。所以这里的
/// fixture 必须两种都造：只造前一种的话，撤掉封顶的钳位这条测试照样是绿的。
#[test]
fn an_out_of_range_window_hours_does_not_take_the_whole_project_down() {
    for hours in ["99999999999", "-99999999999", "999999999999999999", "9223372036854775807"] {
        let hours_n: i64 = hours.parse().unwrap();
        // quota_full=false：配额还空着，走 `window_span`；true：配额占满，多走一次等待封顶
        for quota_full in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let d = dir.path();
            assert_eq!(zloop(d, &["init", "alpha"], None, &[]).code, 0);
            zloop(d, &["plan"], Some("[P1] one\n[P2] two\n"), &[]);
            if quota_full {
                let o = zloop(d, &["done", "t1", "--note", "x", "--outcome", "progress", "--no-doc"], None, &[]);
                assert_eq!(o.code, 0, "{}{}", o.out, o.err);
            }
            let p = state::state_path(d);
            let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            v["policy"]["window_hours"] = serde_json::json!(hours_n);
            if quota_full {
                v["policy"]["max_runs"] = serde_json::json!(1);
            }
            fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

            // 撤掉钳位时 status / context / next 一起 exit 101
            for args in [vec!["status"], vec!["context"], vec!["next", "--peek", "--json"]] {
                let o = zloop(d, &args, None, &[]);
                let tag = format!("window_hours={hours} quota_full={quota_full}");
                assert_eq!(o.code, 0, "{tag} 时 `zloop {}` 该照常能用：{}{}", args.join(" "), o.out, o.err);
                assert!(!o.err.contains("panicked"), "{tag} `zloop {}`：{}", args.join(" "), o.err);
            }
            // fixture 防空跑：`throttled` 那一支必须真的走到，否则上面三条等于没验封顶那处钳位。
            // 负数被钳成 0 窗口 → 刚写下的那条 tick 落在窗口外，配额本来就是空的，不该 throttle。
            let o = zloop(d, &["next", "--peek", "--json"], None, &[]);
            let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
            let want = if quota_full && hours_n > 0 { "throttled" } else { "ready" };
            assert_eq!(v["reason"], want, "window_hours={hours} quota_full={quota_full}：{}", o.out);
            if want == "throttled" {
                // 封顶按钳过的窗口（一年）算，不是按写在文件里的那个数
                let m = v["interval_min"].as_u64().unwrap_or_else(|| panic!("{}", o.out));
                assert!(m <= 365 * 24 * 60, "等待要封在钳过的窗口以内：{m}");
            }
            // 而且不是闷头钳掉就算了：doctor 得把这个没生效的取值报出来
            let o = zloop(d, &["doctor", "--json"], None, &[]);
            let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
            let kinds: Vec<&str> = v["findings"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
            assert!(kinds.contains(&"bad_policy"), "window_hours={hours} 该被 doctor 报出来：{}", o.out);
        }
    }
}

/// `policy.intervals_min` 越界的 CLI 面：A-7/A-11 的第三次重演，这一次 doctor 一声不吭。
///
/// 复现（修之前）：`intervals_min = [4294967295]` + 一条卡在人手里的 todo →
/// debug 构建 `next --peek --json` / `status` / `context` 全在 `phase.rs` 的
/// `human_minutes` 上 `attempt to add with overflow`，exit 101；release 构建不崩，
/// 但 `interval_min` 是 4294967295 分钟（8171 年，runner 就此睡死），
/// 而面板上因为同一处加法回绕印的是「约 0 天后重试」，`doctor --json` 的 findings 是空的。
/// 三样东西同时说"没事"，这是最难查的那一类。
///
/// 单元测试（tick_test）钉的是钳位本身，这里钉的是"用户真敲的那几条命令还能用、
/// 而且看得出配置写歪了"。
#[test]
fn an_out_of_range_interval_does_not_panic_or_sleep_the_loop_forever() {
    let cap = zloop::tick::INTERVAL_MIN_MAX;
    for intervals in [
        serde_json::json!([4_294_967_295u32]),
        serde_json::json!([0]),
        serde_json::json!([3, 10, 4_294_967_295u32]), // 只歪最慢那一档
    ] {
        // user_gate=true 时那条 todo 卡在人手里（原始复现的分支）；false 时是正常派活
        for user_gate in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let d = dir.path();
            assert_eq!(zloop(d, &["init", "alpha"], None, &[]).code, 0);
            zloop(d, &["plan"], Some("[P1] one\n"), &[]);
            if user_gate {
                let o = zloop(d, &["done", "t1", "--note", "x", "--block", "which db?", "--no-doc"], None, &[]);
                assert_eq!(o.code, 0, "{}{}", o.out, o.err);
            }
            let p = state::state_path(d);
            let mut v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            v["policy"]["intervals_min"] = intervals.clone();
            fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();

            let tag = format!("intervals_min={intervals} user_gate={user_gate}");
            for args in [vec!["status"], vec!["context"], vec!["next", "--peek", "--json"]] {
                let o = zloop(d, &args, None, &[]);
                assert_eq!(o.code, 0, "{tag} 时 `zloop {}` 该照常能用：{}{}", args.join(" "), o.out, o.err);
                assert!(!o.err.contains("panicked"), "{tag} `zloop {}`：{}", args.join(" "), o.err);
            }
            // fixture 防空跑：user_gate 那一支必须真的走到，否则等于只验了 ready
            let o = zloop(d, &["next", "--peek", "--json"], None, &[]);
            let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
            assert_eq!(v["reason"], if user_gate { "user_gate" } else { "ready" }, "{tag}：{}", o.out);
            let m = v["interval_min"].as_u64().unwrap_or_else(|| panic!("{tag}：{}", o.out));
            assert!(m >= 1 && m <= cap as u64, "{tag}：间隔要钳进 1..={cap}，实际 {m}");

            // 而且不是闷头钳掉就算了：doctor 得把这个没生效的取值报出来（修之前 findings 是空的）
            let o = zloop(d, &["doctor", "--json"], None, &[]);
            let v: serde_json::Value = serde_json::from_str(&o.out).unwrap();
            let kinds: Vec<&str> = v["findings"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
            assert!(kinds.contains(&"bad_policy"), "{tag} 该被 doctor 报出来：{}", o.out);
        }
    }
}
