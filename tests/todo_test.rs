mod common;

use zloop::state;
use zloop::todo;

#[test]
fn parse_line_variants() {
    assert_eq!(todo::parse_line("[P0] do it", 1), Some((0, "do it".into())));
    assert_eq!(todo::parse_line("- [p2]   spaced ", 1), Some((2, "spaced".into())));
    assert_eq!(todo::parse_line("* plain bullet", 1), Some((1, "plain bullet".into())));
    assert_eq!(todo::parse_line("no prefix", 1), Some((1, "no prefix".into())));
    assert_eq!(todo::parse_line("# comment", 1), None);
    assert_eq!(todo::parse_line("   ", 1), None);
    assert_eq!(todo::parse_line("[P0]", 1), None);
    assert_eq!(todo::parse_line("[x] keep brackets", 1), Some((1, "[x] keep brackets".into())));
}

#[test]
fn parse_plan_skips_blank_and_comments() {
    let items = todo::parse_plan("# plan\n\n[P0] a\n[P1] b\n", 1);
    assert_eq!(items, vec![(0, "a".to_string()), (1, "b".to_string())]);
}

#[test]
fn add_assigns_sequential_ids_and_replace_keeps_done() {
    let mut st = state::default_state("g", "g");
    todo::add(&mut st, &[(0, "a".into()), (1, "b".into())], false);
    let ids: Vec<&str> = st.todos.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2"]);
    todo::set_status(&mut st, "t1", "done", None).unwrap();
    todo::add(&mut st, &[(2, "c".into())], true);
    let pairs: Vec<(&str, &str)> = st.todos.iter().map(|t| (t.id.as_str(), t.status.as_str())).collect();
    assert_eq!(pairs, [("t1", "done"), ("t3", "open")]);
    assert_eq!(st.next_id, 4);
}

#[test]
fn open_ordered_and_executable() {
    let mut st = state::default_state("g", "g");
    todo::add(&mut st, &[(1, "b".into()), (0, "c".into()), (0, "a".into()), (2, "z".into())], false);
    todo::set_status(&mut st, "t4", "deferred", None).unwrap();
    let texts: Vec<&str> = todo::open_ordered(&st).iter().map(|&i| st.todos[i].text.as_str()).collect();
    assert_eq!(texts, ["c", "a", "b"]);
    st.todos[2].blocked_by = vec!["t1".into()]; // "a" waits on "b"
    let exec: Vec<&str> = todo::executable(&st).iter().map(|&i| st.todos[i].text.as_str()).collect();
    assert_eq!(exec, ["c", "b"]);
    assert_eq!(todo::remaining(&st), 3);
}

#[test]
fn set_status_open_clears_user_marker_and_done_at() {
    let mut st = state::default_state("g", "g");
    todo::add(&mut st, &[(0, "a".into())], false);
    todo::set_status(&mut st, "t1", "blocked", Some("why")).unwrap();
    st.todos[0].blocked_by = vec!["user".into(), "t9".into()];
    todo::set_status(&mut st, "t1", "open", None).unwrap();
    assert_eq!(st.todos[0].blocked_by, vec!["t9".to_string()]);
    assert_eq!(st.todos[0].note, "why");
    todo::set_status(&mut st, "t1", "done", None).unwrap();
    assert!(st.todos[0].done_at.is_some());
    assert!(todo::set_status(&mut st, "t1", "bogus", None).is_err());
    assert!(todo::index_of(&st, "t99").is_err());
}

#[test]
fn insert_after_inherits_priority() {
    let mut st = state::default_state("g", "g");
    todo::add(&mut st, &[(0, "a".into()), (2, "z".into())], false);
    let new = todo::insert_after(&mut st, "t1", "b", None).unwrap();
    assert_eq!(new.priority, 0);
    let ids: Vec<&str> = st.todos.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["t1", "t3", "t2"]);
}

const LOOPX_STATE: &str = r#"---
status: active
objective: "demo"
---
## User Todo / Owner Review Reading Queue

## Agent Todo

- [x] [P1] Run `loopx check` against the project registry.
  <!-- loopx:todo todo_id=todo_fa50 status=done task_class=advancement_task -->
- [ ] [P0] 研读 loopx 核心调度链路，产出 notes
  <!-- loopx:todo todo_id=todo_6b30 status=open claimed_by=agent-a1 -->
- [ ] [P2] README + 迁移说明 <!-- loopx:todo todo_id=todo_fddd status=open -->
- [-] [P1] deferred thing
  <!-- loopx:todo todo_id=todo_dead status=deferred -->

## Next Action

- [P0] 研读 loopx 核心调度链路，产出 notes
<!-- loopx:next-action schema=loopx_next_action_binding_v0 todo_id=todo_6b30 -->
"#;

#[test]
fn parse_loopx_state_keeps_only_open_items() {
    assert_eq!(
        todo::parse_loopx_state(LOOPX_STATE),
        vec![(0, "研读 loopx 核心调度链路，产出 notes".to_string()), (2, "README + 迁移说明".to_string())]
    );
}

#[test]
fn plan_line_acceptance_syntax() {
    let mut st = state::default_state("g", "g");
    let items = todo::parse_plan("[P0] build api :: all CRUD endpoints return 200 and tests pass\n[P1] no acceptance here\n[P2] weird :: \n", 1);
    todo::add(&mut st, &items, false);
    assert_eq!(st.todos[0].text, "build api");
    assert_eq!(st.todos[0].acceptance.as_deref(), Some("all CRUD endpoints return 200 and tests pass"));
    assert_eq!(st.todos[1].acceptance, None);
    assert!(st.todos[2].acceptance.is_none());
    assert_eq!(todo::split_acceptance("x :: y"), ("x".to_string(), Some("y".to_string())));
    assert_eq!(todo::split_acceptance(" :: y"), (":: y".to_string(), None));
}
