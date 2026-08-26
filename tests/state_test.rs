mod common;

use std::fs;
use std::thread;
use std::time::Duration;
use zloop::state::{self, StateError};

#[test]
fn roundtrip_and_atomic_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = state::state_path(dir.path());
    let mut st = state::default_state("goal", "proj");
    state::save(&path, &mut st).unwrap();
    assert!(!path.with_file_name("state.json.tmp").exists());
    let loaded = state::load(&path).unwrap();
    assert_eq!(loaded.goal.text, "goal");
    assert_eq!(loaded.version, state::VERSION);
    assert_eq!(loaded.policy, state::Policy::default());
}

#[test]
fn load_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = state::state_path(dir.path());
    let err = state::load(&path).unwrap_err();
    assert!(err.downcast_ref::<StateError>().is_some());
    assert!(err.to_string().contains("no zloop state"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{not json").unwrap();
    assert!(state::load(&path).unwrap_err().to_string().contains("corrupt"));
    fs::write(&path, r#"{"version": 99}"#).unwrap();
    assert!(state::load(&path).unwrap_err().to_string().contains("version"));
    fs::write(&path, r#"{"version": 1, "goal": {}}"#).unwrap();
    assert!(state::load(&path).unwrap_err().to_string().contains("missing keys"));
}

#[test]
fn policy_defaults_are_filled_in_and_unknown_keys_survive() {
    let dir = tempfile::tempdir().unwrap();
    let path = state::state_path(dir.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // A file the Python implementation could have written, plus an unknown key.
    fs::write(
        &path,
        r#"{"version":1,"goal":{"id":"p","text":"g","status":"active","created_at":"2026-08-26T22:54:38+08:00"},
            "policy":{"max_runs":5},"todos":[],"ticks":[],"next_id":1,"custom_key":{"a":1}}"#,
    )
    .unwrap();
    let mut loaded = state::load(&path).unwrap();
    assert_eq!(loaded.policy.max_runs, 5);
    assert_eq!(loaded.policy.window_hours, 24);
    state::save(&path, &mut loaded).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"custom_key\""));
}

#[test]
fn find_root_walks_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = state::default_state("g", "p");
    state::save(&state::state_path(dir.path()), &mut st).unwrap();
    let nested = dir.path().join("a").join("b");
    fs::create_dir_all(&nested).unwrap();
    assert_eq!(state::find_root(Some(&nested)), dir.path().canonicalize().unwrap());
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(state::find_root(Some(empty.path())), empty.path().canonicalize().unwrap());
}

#[test]
fn transaction_serializes_concurrent_writers() {
    let dir = tempfile::tempdir().unwrap();
    let path = state::state_path(dir.path());
    let mut st = state::default_state("g", "p");
    state::save(&path, &mut st).unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let p = path.clone();
            thread::spawn(move || {
                for _ in 0..20 {
                    state::transaction(&p, |s| {
                        s.next_id += 1;
                        Ok(())
                    })
                    .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(state::load(&path).unwrap().next_id, 1 + 80);
}

#[test]
fn lock_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = state::state_path(dir.path());
    let mut st = state::default_state("g", "p");
    state::save(&path, &mut st).unwrap();
    let p2 = path.clone();
    state::locked(&path, Duration::from_secs(5), || {
        // A second process-like holder: use a thread so the flock is a different fd.
        let h = thread::spawn(move || state::locked(&p2, Duration::from_millis(200), || Ok(())));
        let err = h.join().unwrap().unwrap_err();
        assert!(err.to_string().contains("could not lock"), "{err}");
        Ok(())
    })
    .unwrap();
}

#[test]
fn parse_iso_accepts_python_formats() {
    assert!(state::parse_iso("2026-08-26T22:54:38+08:00").is_ok());
    assert!(state::parse_iso("2026-08-26T22:54:38.123456+08:00").is_ok());
    assert!(state::parse_iso("2026-08-26T22:54:38").is_ok());
    assert!(state::parse_iso("garbage").is_err());
}
