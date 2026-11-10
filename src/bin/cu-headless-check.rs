//! Explicit test only: monitor focus while the adjacent helper targets its own child.
//! This tool never accepts an external PID, window, or coordinate.
use mac_computer_use::strict_json;
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::{fs::DirBuilderExt, fs::OpenOptionsExt};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
const LIMIT: u64 = 128 * 1024;
enum Message {
    Watch(Result<Option<Value>, String>),
    Trial(Result<Option<Value>, String>),
}
fn read_line<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    let n = (&mut *reader)
        .take(LIMIT + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "read_failed")?;
    if n == 0 {
        return Ok(None);
    }
    if n as u64 > LIMIT || bytes.last() != Some(&b'\n') {
        return Err("invalid_or_oversized_line".into());
    }
    strict_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "invalid_json".into())
}
fn reader<R: Read + Send + 'static>(input: R, tx: mpsc::Sender<Message>, watch: bool) {
    thread::spawn(move || {
        let mut r = BufReader::new(input);
        loop {
            let result = read_line(&mut r);
            let terminal = !matches!(&result, Ok(Some(_)));
            if tx
                .send(if watch {
                    Message::Watch(result)
                } else {
                    Message::Trial(result)
                })
                .is_err()
                || terminal
            {
                break;
            }
        }
    });
}
fn kill_and_wait(child: &mut Child) -> bool {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    child.wait().is_ok()
}
fn fixed_program(base: &Path, name: &str) -> Result<PathBuf, String> {
    let p = base.join(name);
    if !p.is_file() {
        return Err(format!("missing_{name}"));
    }
    Ok(p)
}
fn safe(v: &Value) -> bool {
    v["action_allowed"] == false && v["input_posted"] == false
}
fn trial_ok(v: &Value, parent_pid: u32, program: &Path, expected_front: &Value) -> bool {
    let p = &v["child_pid"];
    let receipt = &v["child_receipt"];
    let final_count = &v["child_final_count"];
    let marker = &v["marker"];
    let nonce = &v["nonce"];
    v["status"] == "experiment_completed"
        && v["expected_foreground_required"] == true
        && v["expected_foreground_matched"] == true
        && v["foreground_identity"] == *expected_front
        && v["experiment_scope"] == "own_child_process_only"
        && v["general_input_authorized"] == false
        && v["action_attempted"] == true
        && v["child_received_one_tagged_event_in_observation_window"] == true
        && v["sampled_state_unchanged"] == true
        && v["post_sampling_coverage"] == true
        && v["child_stopped"] == true
        && v["cleanup_status"] == "completed"
        && v["sample_errors"] == 0
        && v["sample_count"].as_u64().is_some_and(|n| n >= 3)
        && [
            "available",
            "cursor_unchanged",
            "focused_element_unchanged",
            "frontmost_unchanged",
            "modifier_flags_clear",
            "mouse_buttons_clear",
            "keyboard_keys_clear",
            "window_order_unchanged",
        ]
        .iter()
        .all(|k| v["sample_diagnostics"][*k] == true)
        && v["child_process_start_unix_seconds"]
            .as_i64()
            .is_some_and(|n| n > 0)
        && v["child_process_start_microseconds"]
            .as_i64()
            .is_some_and(|n| (0..1_000_000).contains(&n))
        && p.as_u64().is_some_and(|n| n > 0)
        && marker.as_i64().is_some_and(|n| n > 0)
        && nonce.as_str().is_some_and(|s| !s.is_empty())
        && v["child_executable"].as_str() == program.to_str()
        && receipt["child_pid"] == *p
        && receipt["sender_pid"] == parent_pid
        && receipt["kind"] == "mouse_moved"
        && receipt["count"] == 1
        && receipt["marker"] == *marker
        && receipt["nonce"] == *nonce
        && final_count["child_pid"] == *p
        && final_count["count"] == 1
        && final_count["marker"] == *marker
        && final_count["nonce"] == *nonce
}
fn save(path: &Path, value: &Value) -> Result<(), String> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    o.mode(0o600);
    let mut f = o.open(path).map_err(|_| "result_create_failed")?;
    let b = serde_json::to_vec_pretty(value).map_err(|_| "result_encode_failed")?;
    f.write_all(&b)
        .and_then(|_| f.sync_all())
        .map_err(|_| "result_write_failed".into())
}
fn run(dir: &Path) -> Result<bool, String> {
    if !dir.is_absolute() {
        return Err("absolute_new_directory_required".into());
    }
    let exe = env::current_exe().map_err(|_| "executable_unavailable")?;
    let base = exe.parent().ok_or("executable_directory_missing")?;
    let watch = fixed_program(base, "cu-focus-watch")?;
    let helper = fixed_program(base, "mac-focus-notifications")?;
    let trial = fixed_program(base, "mac-headless-event-experiment")?
        .canonicalize()
        .map_err(|_| "trial_path_unavailable")?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(dir)
        .map_err(|_| "cannot_create_evidence_directory")?;
    let mut result = json!({"status":"stopped","action_allowed":false,"general_input_authorized":false,"input_isolation_verified":false,
        "trial_started":false,"trial_delivery":"not_attempted","watch_records":[],"trial_record":null,
        "note":"Explicit own-child test only. Focus notifications and samples can miss changes; neither proves input isolation."});
    let (tx, rx) = mpsc::channel();
    let mut watcher = match Command::new(watch)
        .arg("--helper")
        .arg(helper)
        .arg("--evidence")
        .arg(dir.join("focus.jsonl"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(x) => x,
        Err(_) => {
            result["reason"] = "watch_start_failed".into();
            save(&dir.join("result.json"), &result)?;
            return Ok(false);
        }
    };
    let watch_stdout = watcher.stdout.take().ok_or("watch_stdout_unavailable")?;
    reader(watch_stdout, tx.clone(), true);
    let mut trial_process: Option<Child> = None;
    let mut trial_exit: Option<i32> = None;
    let mut trial_finished = false;
    let mut trial_record: Option<Value> = None;
    let mut watch_identity: Option<Value> = None;
    let mut watch_ready = false;
    let mut watch_done = false;
    let mut watch_eof = false;
    let mut trial_eof = false;
    let mut sent_stop = false;
    let mut bad = false;
    let mut reason = "deadline".to_string();
    let started = Instant::now();
    let deadline = started + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(c) = trial_process.as_mut() {
            if !trial_finished {
                if let Ok(Some(status)) = c.try_wait() {
                    trial_exit = status.code();
                    trial_finished = true;
                }
            }
        }
        if trial_finished && trial_eof && !sent_stop && !watch_done {
            match watcher
                .stdin
                .as_mut()
                .and_then(|s| s.write_all(b"stop\n").and_then(|_| s.flush()).ok())
            {
                Some(()) => sent_stop = true,
                None => {
                    bad = true;
                    reason = "watch_stop_failed".into();
                    break;
                }
            }
        }
        if watch_done && watch_eof && (trial_process.is_none() || (trial_finished && trial_eof)) {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(Message::Watch(Ok(Some(v)))) => {
                if let Some(a) = result["watch_records"].as_array_mut() {
                    a.push(v.clone())
                }
                if !safe(&v) {
                    bad = true;
                    reason = "unsafe_watch_record".into();
                    break;
                }
                match v["status"].as_str() {
                    Some("ready")
                        if !watch_ready
                            && v["scope"] == "frontmost_app"
                            && v["foreground_identity"]["pid"]
                                .as_u64()
                                .is_some_and(|n| n > 0 && n <= i32::MAX as u64)
                            && v["foreground_identity"]["started_seconds"]
                                .as_i64()
                                .is_some_and(|n| n > 0)
                            && v["foreground_identity"]["started_microseconds"]
                                .as_u64()
                                .is_some_and(|n| n < 1_000_000) =>
                    {
                        watch_identity = Some(v["foreground_identity"].clone());
                        watch_ready = true;
                        // Drain already queued watch output before starting. A change may still race the post.
                        match rx.try_recv() {
                            Ok(Message::Watch(Ok(Some(extra)))) => {
                                result["watch_records"].as_array_mut().unwrap().push(extra);
                                bad = true;
                                reason = "watch_changed_before_trial".into();
                                break;
                            }
                            Ok(_) => {
                                bad = true;
                                reason = "watch_unavailable_before_trial".into();
                                break;
                            }
                            Err(mpsc::TryRecvError::Empty) => (),
                            Err(_) => {
                                bad = true;
                                reason = "watch_unavailable_before_trial".into();
                                break;
                            }
                        }
                        if !matches!(watcher.try_wait(), Ok(None)) {
                            bad = true;
                            reason = "watch_unavailable_before_trial".into();
                            break;
                        }
                        let expected = match watch_identity
                            .as_ref()
                            .and_then(|v| serde_json::to_string(v).ok())
                        {
                            Some(v) => v,
                            None => {
                                bad = true;
                                reason = "watch_identity_unavailable".into();
                                break;
                            }
                        };
                        // Write before spawn: absence of a final report after a crash
                        // must never be read as proof that nothing was sent.
                        let marker = json!({"status":"delivery_unknown_if_no_final_result", "action_allowed":false,
                            "scope":"own_child_test_only", "note":"Written before spawn; does not prove the trial started or posted input."});
                        if save(&dir.join("attempt.json"), &marker).is_err() {
                            bad = true;
                            reason = "attempt_marker_write_failed".into();
                            break;
                        }
                        result["attempt_marker"] = "attempt.json".into();
                        // Give the monitor a bounded chance to report a change that
                        // occurred during the marker write. Delivery can still race.
                        match rx.recv_timeout(Duration::from_millis(150)) {
                            Ok(Message::Watch(Ok(Some(extra)))) => {
                                result["watch_records"].as_array_mut().unwrap().push(extra);
                                bad = true;
                                reason = "watch_changed_before_trial".into();
                                break;
                            }
                            Ok(_) => {
                                bad = true;
                                reason = "watch_unavailable_before_trial".into();
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => (),
                            Err(_) => {
                                bad = true;
                                reason = "watch_unavailable_before_trial".into();
                                break;
                            }
                        }
                        if !matches!(watcher.try_wait(), Ok(None)) {
                            bad = true;
                            reason = "watch_unavailable_before_trial".into();
                            break;
                        }
                        match Command::new(&trial)
                            .arg("--run-experiment")
                            .env("CU_EXPECTED_FOREGROUND_IDENTITY", expected)
                            .stdin(Stdio::null())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::null())
                            .spawn()
                        {
                            Ok(mut c) => {
                                result["trial_started"] = true.into();
                                result["trial_delivery"] = "unknown".into();
                                if let Some(out) = c.stdout.take() {
                                    reader(out, tx.clone(), false);
                                    trial_process = Some(c)
                                } else {
                                    trial_process = Some(c);
                                    bad = true;
                                    reason = "trial_stdout_unavailable".into();
                                    break;
                                }
                            }
                            Err(_) => {
                                bad = true;
                                reason = "trial_start_failed".into();
                                break;
                            }
                        }
                    }
                    Some("focus_changed") => {
                        bad = true;
                        reason = "focus_changed".into();
                        if let Some(c) = trial_process.as_mut() {
                            let _ = c.kill();
                        }
                        break;
                    }
                    Some("stopped")
                        if sent_stop
                            && v["reason"] == "requested"
                            && v["scope"] == "frontmost_app"
                            && v["helper_exit_code"] == 0
                            && v["frontmost_same_at_end"] == true
                            && v["input_isolation_verified"] == false
                            && watch_identity.as_ref() == Some(&v["foreground_identity"])
                            && ["focused_element", "focused_window", "deactivated"]
                                .iter()
                                .all(|k| v["counts"][*k] == 0) =>
                    {
                        watch_done = true;
                    }
                    _ => {
                        bad = true;
                        reason = "watch_stopped_or_invalid".into();
                        if let Some(c) = trial_process.as_mut() {
                            let _ = c.kill();
                        }
                        break;
                    }
                }
            }
            Ok(Message::Watch(Ok(None))) => {
                watch_eof = true;
                if !watch_done {
                    bad = true;
                    reason = "watch_closed_early".into();
                    if let Some(c) = trial_process.as_mut() {
                        let _ = c.kill();
                    }
                    break;
                }
            }
            Ok(Message::Watch(Err(_))) => {
                bad = true;
                reason = "watch_output_invalid".into();
                break;
            }
            Ok(Message::Trial(Ok(Some(v)))) => {
                if trial_record.is_some() {
                    bad = true;
                    reason = "multiple_trial_records".into();
                    break;
                }
                trial_record = Some(v.clone());
                result["trial_record"] = v;
            }
            Ok(Message::Trial(Ok(None))) => {
                trial_eof = true;
            }
            Ok(Message::Trial(Err(_))) => {
                bad = true;
                reason = "trial_output_invalid".into();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout)
                if !watch_ready && started.elapsed() >= Duration::from_secs(5) =>
            {
                bad = true;
                reason = "watch_ready_timeout".into();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => {
                bad = true;
                reason = "observation_disconnected".into();
                break;
            }
        }
    }
    if Instant::now() >= deadline {
        bad = true;
        reason = "deadline".into()
    }
    let trial_pid = trial_process.as_ref().map(|c| c.id());
    let trial_stopped = trial_process.as_mut().map(kill_and_wait).unwrap_or(true);
    let watch_deadline = Instant::now() + Duration::from_secs(1);
    let mut watch_exit = None;
    while Instant::now() < watch_deadline {
        match watcher.try_wait() {
            Ok(Some(status)) => {
                watch_exit = status.code();
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    let watch_stopped = kill_and_wait(&mut watcher);
    let completed = !bad
        && watch_ready
        && watch_done
        && watch_eof
        && sent_stop
        && watch_exit == Some(0)
        && trial_exit == Some(0)
        && trial_eof
        && trial_pid
            .zip(trial_record.as_ref())
            .zip(watch_identity.as_ref())
            .is_some_and(|((pid, v), front)| trial_ok(v, pid, &trial, front));
    if completed {
        result["status"] = "experiment_completed".into();
        result["trial_delivery"] = "one_tagged_event_to_own_child".into();
        result["reason"] = "completed".into()
    } else {
        if reason == "deadline" && Instant::now() < deadline {
            reason = "trial_or_watch_incomplete".into()
        }
        result["reason"] = reason.into()
    }
    result["watch_process_stopped"] = watch_stopped.into();
    result["trial_process_stopped"] = trial_stopped.into();
    result["trial_descendant_cleanup"] = if completed {
        "reported_completed"
    } else {
        "unverified"
    }
    .into();
    result["trial_exit_code"] = json!(trial_exit);
    result["watch_exit_code"] = json!(watch_exit);
    save(&dir.join("result.json"), &result)?;
    println!("{}", result);
    Ok(completed)
}
fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 || args[1] != "--evidence-dir" {
        eprintln!("usage: cu-headless-check --evidence-dir /absolute/new-directory");
        std::process::exit(2)
    }
    match run(Path::new(&args[2])) {
        Ok(true) => (),
        Ok(false) => std::process::exit(3),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_and_diagnostics_must_all_match() {
        let mut v = json!({"status":"experiment_completed","expected_foreground_required":true,"expected_foreground_matched":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"experiment_scope":"own_child_process_only","general_input_authorized":false,"action_attempted":true,
            "child_received_one_tagged_event_in_observation_window":true,"sampled_state_unchanged":true,"post_sampling_coverage":true,
            "child_stopped":true,"cleanup_status":"completed","sample_errors":0,"sample_count":40,
            "sample_diagnostics":{"available":true,"cursor_unchanged":true,"focused_element_unchanged":true,"frontmost_unchanged":true,"modifier_flags_clear":true,"mouse_buttons_clear":true,"keyboard_keys_clear":true,"window_order_unchanged":true},
            "child_process_start_unix_seconds":123,"child_process_start_microseconds":42,"child_pid":5,"marker":7,"nonce":"n","child_executable":"/test",
            "child_receipt":{"child_pid":5,"sender_pid":4,"kind":"mouse_moved","count":1,"marker":7,"nonce":"n"},
            "child_final_count":{"child_pid":5,"count":1,"marker":7,"nonce":"n"}});
        assert!(trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":123,"started_microseconds":4})
        ));
        v["sample_diagnostics"]["keyboard_keys_clear"] = false.into();
        assert!(!trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":123,"started_microseconds":4})
        ));
        v["sample_diagnostics"]
            .as_object_mut()
            .unwrap()
            .remove("keyboard_keys_clear");
        assert!(!trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":123,"started_microseconds":4})
        ));
        v["sample_diagnostics"]["keyboard_keys_clear"] = true.into();
        assert!(!trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":124,"started_microseconds":4})
        ));
        v["sample_diagnostics"]["focused_element_unchanged"] = false.into();
        assert!(!trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":123,"started_microseconds":4})
        ));
        v["sample_diagnostics"]["focused_element_unchanged"] = true.into();
        v["child_final_count"]["marker"] = 8.into();
        assert!(!trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":123,"started_microseconds":4})
        ));
        v["child_final_count"]["marker"] = 7.into();
        v["experiment_scope"] = "other".into();
        assert!(!trial_ok(
            &v,
            4,
            Path::new("/test"),
            &json!({"pid":10,"started_seconds":123,"started_microseconds":4})
        ));
    }
}
