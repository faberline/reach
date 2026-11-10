#![cfg(unix)]
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn setup(watch: &str, trial: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "headless-check-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let exe = bin.join("check");
    fs::copy(env!("CARGO_BIN_EXE_cu-headless-check"), &exe).unwrap();
    for (name, body) in [
        ("cu-focus-watch", watch),
        ("mac-headless-event-experiment", trial),
        ("mac-focus-notifications", "exit 0"),
    ] {
        let path = bin.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    (root, exe)
}
fn run(root: &Path, exe: &PathBuf) -> (std::process::Output, Value) {
    let dir = root.join("evidence");
    let out = Command::new(exe)
        .arg("--evidence-dir")
        .arg(&dir)
        .output()
        .unwrap();
    let result = serde_json::from_slice(&fs::read(dir.join("result.json")).unwrap()).unwrap();
    (out, result)
}
const READY: &str = r#"{"status":"ready","foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"scope":"frontmost_app","action_allowed":false,"input_posted":false}"#;
const FINAL: &str = r#"{"status":"stopped","foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"scope":"frontmost_app","reason":"requested","action_allowed":false,"input_posted":false,"input_isolation_verified":false,"helper_exit_code":0,"frontmost_same_at_end":true,"counts":{"focused_element":0,"focused_window":0,"deactivated":0}}"#;
#[test]
fn no_ready_never_starts_the_test() {
    let (d,exe)=setup("printf '%s\\n' '{\"status\":\"error\",\"action_allowed\":false,\"input_posted\":false}'\nexit 3","exit 0");
    let (out, v) = run(&d, &exe);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(v["trial_started"], false);
    assert_eq!(v["trial_delivery"], "not_attempted");
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn existing_marker_prevents_trial_start_and_is_not_overwritten() {
    let watcher=format!("printf existing > \"$(dirname \"$4\")/attempt.json\"\nprintf '%s\\n' '{READY}'\nread command");
    let trial = "touch \"$0.started\"";
    let (d, exe) = setup(&watcher, trial);
    let (out, v) = run(&d, &exe);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(v["reason"], "attempt_marker_write_failed");
    assert_eq!(v["trial_started"], false);
    assert_eq!(v["trial_delivery"], "not_attempted");
    assert_eq!(
        fs::read(d.join("evidence/attempt.json")).unwrap(),
        b"existing"
    );
    assert!(!d.join("bin/mac-headless-event-experiment.started").exists());
    assert!(!d.join("bin/mac-hidden-appkit-experiment.started").exists());
    assert!(!d.join("bin/mac-hidden-window-experiment.started").exists());
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn focus_change_after_marker_stops_before_trial() {
    let changed = r#"{"status":"focus_changed","kind":"deactivated","action_allowed":false,"input_posted":false}"#;
    let watcher=format!("printf '%s\\n' '{READY}'\nwhile [ ! -e \"$(dirname \"$4\")/attempt.json\" ]; do sleep 0.01; done\nprintf '%s\\n' '{changed}'\nread command");
    let (d, exe) = setup(&watcher, "touch \"$0.started\"");
    let (out, v) = run(&d, &exe);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(v["reason"], "watch_changed_before_trial");
    assert_eq!(v["trial_started"], false);
    assert_eq!(v["trial_delivery"], "not_attempted");
    assert!(d.join("evidence/attempt.json").exists());
    assert!(!d.join("bin/mac-headless-event-experiment.started").exists());
    assert!(!d.join("bin/mac-hidden-appkit-experiment.started").exists());
    assert!(!d.join("bin/mac-hidden-window-experiment.started").exists());
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn monitor_event_stops_test_and_keeps_delivery_unknown() {
    let changed = r#"{"status":"focus_changed","kind":"deactivated","action_allowed":false,"input_posted":false}"#;
    let (d, exe) = setup(
        &format!("printf '%s\\n' '{READY}'\nsleep 0.3\nprintf '%s\\n' '{changed}'\nexit 3"),
        "exec sleep 5",
    );
    let (out, v) = run(&d, &exe);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(v["reason"], "focus_changed");
    assert_eq!(v["trial_started"], true);
    assert_eq!(v["trial_delivery"], "unknown");
    assert_eq!(v["trial_descendant_cleanup"], "unverified");
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn invalid_trial_output_keeps_delivery_unknown() {
    let watcher = format!("printf '%s\\n' '{READY}'\nread command\nprintf '%s\\n' '{FINAL}'");
    let (d, exe) = setup(&watcher, "printf '%s\\n' 'not-json'");
    let (out, v) = run(&d, &exe);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(v["trial_started"], true);
    assert_eq!(v["trial_delivery"], "unknown");
    let marker: Value =
        serde_json::from_slice(&fs::read(d.join("evidence/attempt.json")).unwrap()).unwrap();
    assert_eq!(marker["status"], "delivery_unknown_if_no_final_result");
    assert_eq!(marker["action_allowed"], false);
    assert_eq!(v["trial_descendant_cleanup"], "unverified");
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn coordinator_crash_leaves_pre_spawn_marker() {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let watcher = format!("printf '%s\\n' '{READY}'\nread command");
    let (d, exe) = setup(
        &watcher,
        r#"touch "$0.started"
sleep 1
touch "$0.done""#,
    );
    let evidence = d.join("evidence");
    let trial = d.join("bin/mac-headless-event-experiment");
    let started = trial.with_extension("started");
    let done = trial.with_extension("done");
    let mut child = Command::new(exe)
        .arg("--evidence-dir")
        .arg(&evidence)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let did_start = started.exists();
    let _ = child.kill();
    let _ = child.wait();
    let deadline = Instant::now() + Duration::from_secs(3);
    while did_start && !done.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(did_start, "fake trial did not start");
    assert!(done.exists(), "fake trial did not stop");
    let marker: Value =
        serde_json::from_slice(&fs::read(evidence.join("attempt.json")).unwrap()).unwrap();
    assert_eq!(marker["status"], "delivery_unknown_if_no_final_result");
    assert!(!evidence.join("result.json").exists());
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn complete_test_requires_matching_receipt_and_clean_monitor() {
    let watcher = format!("printf '%s\\n' '{READY}'\nread command\nprintf '%s\\n' '{FINAL}'");
    let trial = r#"test -n "$CU_EXPECTED_FOREGROUND_IDENTITY" || exit 5
printf '{"status":"experiment_completed","expected_foreground_required":true,"expected_foreground_matched":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"experiment_scope":"own_child_process_only","general_input_authorized":false,"action_attempted":true,"child_received_one_tagged_event_in_observation_window":true,"sampled_state_unchanged":true,"post_sampling_coverage":true,"child_stopped":true,"cleanup_status":"completed","sample_errors":0,"sample_count":40,"sample_diagnostics":{"available":true,"cursor_unchanged":true,"focused_element_unchanged":true,"frontmost_unchanged":true,"modifier_flags_clear":true,"mouse_buttons_clear":true,"keyboard_keys_clear":true,"window_order_unchanged":true},"child_process_start_unix_seconds":123,"child_process_start_microseconds":1,"child_pid":1111,"marker":44,"nonce":"n","child_executable":"%s","child_receipt":{"child_pid":1111,"sender_pid":%s,"kind":"mouse_moved","count":1,"marker":44,"nonce":"n"},"child_final_count":{"child_pid":1111,"count":1,"marker":44,"nonce":"n"}}\n' "$0" "$$""#;
    let (d, exe) = setup(&watcher, trial);
    let (out, v) = run(&d, &exe);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(v["status"], "experiment_completed");
    assert_eq!(v["action_allowed"], false);
    assert_eq!(v["input_isolation_verified"], false);
    fs::remove_dir_all(d).unwrap();
}
