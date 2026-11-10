#![cfg(unix)]
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn fixture(script: &str) -> (PathBuf, PathBuf) {
    let d = std::env::temp_dir().join(format!(
        "focus-watch-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&d).unwrap();
    let helper = d.join("helper");
    fs::write(&helper, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    (d, helper)
}
fn spawn(d: &Path, helper: &Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_cu-focus-watch"))
        .args([
            "--helper",
            helper.to_str().unwrap(),
            "--evidence",
            d.join("evidence.jsonl").to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}
const READY: &str = r#"{"status":"ready","foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"scope":"frontmost_app","action_allowed":false,"input_posted":false,"max_seconds":30,"registration":{"focused_element":true,"focused_window":true,"deactivated":true}}"#;
#[test]
fn ready_stop_and_evidence_are_preserved() {
    let final_line = r#"{"status":"sampled","action_allowed":false,"input_posted":false,"registration":{"focused_element":true,"focused_window":true,"deactivated":true},"counts":{"focused_element":0,"focused_window":0,"deactivated":0},"frontmost_same_at_end":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"foreground_identity_at_end":{"pid":10,"started_seconds":123,"started_microseconds":4},"termination":"requested"}"#;
    let (d, h) = fixture(&format!(
        "printf '%s\\n' '{READY}'\nread command\nprintf '%s\\n' '{final_line}'"
    ));
    let mut child = spawn(&d, &h);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["status"],
        "ready"
    );
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    let mut last = String::new();
    reader.read_line(&mut last).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        serde_json::from_str::<Value>(&last).unwrap()["reason"],
        "requested"
    );
    let evidence = fs::read_to_string(d.join("evidence.jsonl")).unwrap();
    assert_eq!(evidence.lines().count(), 2);
    assert!(evidence.contains("input_isolation_verified"));
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn event_stops_without_authorizing_input() {
    let event = r#"{"status":"focus_changed","kind":"deactivated","action_allowed":false,"input_posted":false}"#;
    let final_line = r#"{"status":"sampled","action_allowed":false,"input_posted":false,"registration":{"focused_element":true,"focused_window":true,"deactivated":true},"counts":{"focused_element":0,"focused_window":0,"deactivated":1},"frontmost_same_at_end":false,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"foreground_identity_at_end":null,"termination":"focus_changed"}"#;
    let (d, h) = fixture(&format!(
        "printf '%s\\n' '{READY}' '{event}' '{final_line}'\nexit 3"
    ));
    let mut child = spawn(&d, &h);
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut lines = Vec::new();
    for _ in 0..3 {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        lines.push(serde_json::from_str::<Value>(&line).unwrap());
    }
    assert_eq!(child.wait().unwrap().code(), Some(3));
    assert_eq!(lines[1]["status"], "focus_changed");
    assert_eq!(lines[2]["reason"], "focus_changed");
    assert_eq!(lines[2]["action_allowed"], false);
    drop(child.stdin.take());
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn rejects_helper_claiming_authority_and_keeps_evidence() {
    let bad = READY.replace("\"action_allowed\":false", "\"action_allowed\":true");
    let (d, h) = fixture(&format!("printf '%s\\n' '{bad}'"));
    let mut child = spawn(&d, &h);
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(!child.wait().unwrap().success());
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["reason"],
        "unsafe_helper_record"
    );
    drop(child.stdin.take());
    assert!(d.join("evidence.jsonl").exists());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn rejects_records_after_terminal_message() {
    let final_line = r#"{"status":"sampled","action_allowed":false,"input_posted":false,"registration":{"focused_element":true,"focused_window":true,"deactivated":true},"counts":{"focused_element":0,"focused_window":0,"deactivated":0},"frontmost_same_at_end":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"foreground_identity_at_end":{"pid":10,"started_seconds":123,"started_microseconds":4},"termination":"requested"}"#;
    let extra = r#"{"status":"focus_changed","kind":"deactivated","action_allowed":false,"input_posted":false}"#;
    let (d, h) = fixture(&format!(
        "printf '%s\\n' '{READY}'\nread command\nprintf '%s\\n' '{final_line}' '{extra}'"
    ));
    let mut child = spawn(&d, &h);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    let mut last = String::new();
    reader.read_line(&mut last).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&last).unwrap()["reason"],
        "unexpected_trailing_output"
    );
    assert!(!child.wait().unwrap().success());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn explicit_self_test_only_accepts_own_child_scope() {
    let ready = READY
        .replace("frontmost_app", "own_child_test")
        .replace("\"max_seconds\":30", "\"max_seconds\":3");
    let event = r#"{"status":"focus_changed","kind":"focused_element","action_allowed":false,"input_posted":false}"#;
    let final_line = r#"{"status":"sampled","scope":"own_child_test","test_child_stopped":true,"action_allowed":false,"input_posted":false,"registration":{"focused_element":true,"focused_window":true,"deactivated":true},"counts":{"focused_element":1,"focused_window":0,"deactivated":0},"frontmost_same_at_end":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"foreground_identity_at_end":{"pid":10,"started_seconds":123,"started_microseconds":4},"termination":"focus_changed"}"#;
    let (d, h) = fixture(&format!(
        "printf '%s\\n' '{ready}' '{event}' '{final_line}'\nexit 3"
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_cu-focus-watch"))
        .args([
            "--self-test-notification",
            "--helper",
            h.to_str().unwrap(),
            "--evidence",
            d.join("evidence.jsonl").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let records: Vec<Value> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["scope"], "own_child_test");
    assert_eq!(records[1]["kind"], "focused_element");
    assert_eq!(records[2]["reason"], "focus_changed");
    assert_eq!(records[2]["input_isolation_verified"], false);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn rejects_reused_foreground_pid_with_different_start_time() {
    let final_line = r#"{"status":"sampled","action_allowed":false,"input_posted":false,"registration":{"focused_element":true,"focused_window":true,"deactivated":true},"counts":{"focused_element":0,"focused_window":0,"deactivated":0},"frontmost_same_at_end":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"foreground_identity_at_end":{"pid":10,"started_seconds":124,"started_microseconds":4},"termination":"requested"}"#;
    let (d, h) = fixture(&format!(
        "printf '%s\\n' '{READY}'\nread command\nprintf '%s\\n' '{final_line}'"
    ));
    let mut child = spawn(&d, &h);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    child.stdin.take().unwrap().write_all(b"stop\n").unwrap();
    let mut last = String::new();
    reader.read_line(&mut last).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&last).unwrap()["reason"],
        "foreground_identity_inconsistent"
    );
    assert!(!child.wait().unwrap().success());
    fs::remove_dir_all(d).unwrap();
}
