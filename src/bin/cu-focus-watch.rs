//! Read-only supervisor for the focus-notification helper. It never posts input.
use mac_computer_use::strict_json;
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    env,
    fs::OpenOptions,
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const LIMIT: u64 = 8192;
enum Message {
    Helper(Result<Value, String>),
    Control(Result<String, String>),
}
fn read_bounded<R: BufRead>(reader: &mut R) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    let count = (&mut *reader)
        .take(LIMIT + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| "pipe_read_failed")?;
    if count == 0 {
        return Ok(None);
    }
    if count as u64 > LIMIT || bytes.last() != Some(&b'\n') {
        return Err("line_invalid_or_too_long".into());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "invalid_utf8".into())
}
fn safe(value: &Value) -> bool {
    value["action_allowed"] == false && value["input_posted"] == false
}
fn registrations(value: &Value) -> bool {
    let Some(r) = value["registration"].as_object() else {
        return false;
    };
    r.len() == 3
        && ["focused_element", "focused_window", "deactivated"]
            .iter()
            .all(|k| r.get(*k) == Some(&Value::Bool(true)))
}
fn identity(value: &Value) -> bool {
    value.as_object().is_some_and(|v| v.len() == 3)
        && value["pid"]
            .as_u64()
            .is_some_and(|v| v > 0 && v <= i32::MAX as u64)
        && value["started_seconds"].as_i64().is_some_and(|v| v > 0)
        && value["started_microseconds"]
            .as_u64()
            .is_some_and(|v| v < 1_000_000)
}
fn kind(value: &Value) -> Option<&str> {
    value["kind"]
        .as_str()
        .filter(|s| matches!(*s, "focused_element" | "focused_window" | "deactivated"))
}
fn write_record(file: &mut std::fs::File, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| "encode_failed")?;
    bytes.push(b'\n');
    file.write_all(&bytes)
        .and_then(|_| file.flush())
        .map_err(|_| "evidence_write_failed")?;
    io::stdout()
        .write_all(&bytes)
        .and_then(|_| io::stdout().flush())
        .map_err(|_| "stdout_write_failed".into())
}
fn terminate(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
fn run(helper: &Path, evidence: &Path, test_own: bool) -> Result<bool, String> {
    if !helper.is_absolute() || !evidence.is_absolute() {
        return Err("absolute_paths_required".into());
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut file = opts.open(evidence).map_err(|_| "cannot_reserve_evidence")?;
    let mut child = match Command::new(helper)
        .arg(if test_own {
            "--test-own-notification"
        } else {
            "--watch"
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            let _ = write_record(
                &mut file,
                &json!({"status":"error","reason":"helper_start_failed","action_allowed":false}),
            );
            return Err("helper_start_failed".into());
        }
    };
    let mut child_stdin = child.stdin.take().ok_or("helper_stdin_unavailable")?;
    let stdout = child.stdout.take().ok_or("helper_stdout_unavailable")?;
    let (tx, rx) = mpsc::channel();
    let sender = tx.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_bounded(&mut reader) {
                Ok(Some(line)) => {
                    let value = strict_json::from_slice(line.as_bytes())
                        .map_err(|_| "invalid_helper_json".into());
                    if sender.send(Message::Helper(value)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = sender.send(Message::Helper(Err("helper_closed".into())));
                    break;
                }
                Err(e) => {
                    let _ = sender.send(Message::Helper(Err(e)));
                    break;
                }
            }
        }
    });
    if !test_own {
        thread::spawn(move || {
            let input = io::stdin();
            let mut reader = input.lock();
            let msg = match read_bounded(&mut reader) {
                Ok(Some(s)) if s == "stop\n" => Ok(s),
                Ok(None) => Err("control_eof".into()),
                Ok(_) => Err("invalid_control".into()),
                Err(e) => Err(e),
            };
            let _ = tx.send(Message::Control(msg));
        });
    }
    let started = Instant::now();
    let deadline = started + Duration::from_secs(35);
    let mut ready = false;
    let mut baseline_identity: Option<Value> = None;
    let mut stop_sent = false;
    let mut event: Option<String> = None;
    let mut final_ok = false;
    let mut reason = "helper_timeout".to_string();
    let mut final_counts: Option<Value> = None;
    let mut frontmost_same_at_end: Option<bool> = None;
    while Instant::now() < deadline {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        match rx.recv_timeout(remaining) {
            Ok(Message::Control(Ok(_))) if ready && !stop_sent && event.is_none() => {
                if child_stdin
                    .write_all(b"stop\n")
                    .and_then(|_| child_stdin.flush())
                    .is_err()
                {
                    reason = "stop_write_failed".into();
                    break;
                }
                stop_sent = true;
            }
            Ok(Message::Control(Ok(_))) => {
                reason = "control_out_of_order".into();
                break;
            }
            Ok(Message::Control(Err(e))) => {
                reason = e;
                break;
            }
            Ok(Message::Helper(Err(e))) => {
                reason = e;
                break;
            }
            Ok(Message::Helper(Ok(value))) => {
                if !safe(&value) {
                    reason = "unsafe_helper_record".into();
                    break;
                }
                match value["status"].as_str() {
                    Some("ready")
                        if !ready
                            && registrations(&value)
                            && identity(&value["foreground_identity"])
                            && value["scope"]
                                == if test_own {
                                    "own_child_test"
                                } else {
                                    "frontmost_app"
                                }
                            && value["max_seconds"] == if test_own { 3 } else { 30 } =>
                    {
                        ready = true;
                        baseline_identity = Some(value["foreground_identity"].clone());
                        if write_record(&mut file, &json!({"status":"ready","scope":if test_own {"own_child_test"} else {"frontmost_app"},"foreground_identity":baseline_identity,"action_allowed":false,"input_posted":false})).is_err() { reason="evidence_write_failed".into(); break; }
                    }
                    Some("focus_changed") if ready && event.is_none() && kind(&value).is_some() => {
                        event = kind(&value).map(str::to_owned);
                        if write_record(&mut file, &json!({"status":"focus_changed","kind":event,"action_allowed":false,"input_posted":false})).is_err() { reason="evidence_write_failed".into(); break; }
                    }
                    Some("sampled")
                        if ready
                            && registrations(&value)
                            && (!test_own
                                || (value["scope"] == "own_child_test"
                                    && value["test_child_stopped"] == true)) =>
                    {
                        let counts = &value["counts"];
                        let Some(map) = counts.as_object().filter(|m| m.len() == 3) else {
                            reason = "invalid_final_counts".into();
                            break;
                        };
                        let names = ["focused_element", "focused_window", "deactivated"];
                        if names
                            .iter()
                            .any(|k| map.get(*k).and_then(Value::as_u64).is_none())
                            || !value["frontmost_same_at_end"].is_boolean()
                            || baseline_identity.as_ref() != Some(&value["foreground_identity"])
                        {
                            reason = "invalid_final_record".into();
                            break;
                        }
                        let end = &value["foreground_identity_at_end"];
                        if (!end.is_null() && !identity(end))
                            || (value["frontmost_same_at_end"] == true)
                                != (!end.is_null() && baseline_identity.as_ref() == Some(end))
                        {
                            reason = "foreground_identity_inconsistent".into();
                            break;
                        }
                        let observed_event =
                            names.iter().any(|k| counts[*k].as_u64().unwrap_or(0) > 0);
                        final_counts = Some(counts.clone());
                        frontmost_same_at_end = value["frontmost_same_at_end"].as_bool();
                        if event
                            .as_deref()
                            .and_then(|name| counts[name].as_u64())
                            .is_some_and(|n| n > 0)
                            && observed_event
                            && value["termination"] == "focus_changed"
                        {
                            reason = "focus_changed".into();
                            final_ok = true;
                            break;
                        }
                        if !test_own
                            && stop_sent
                            && event.is_none()
                            && !observed_event
                            && value["termination"] == "requested"
                            && value["frontmost_same_at_end"] == true
                        {
                            reason = "requested".into();
                            final_ok = true;
                            break;
                        }
                        reason = "inconsistent_final_record".into();
                        break;
                    }
                    _ => {
                        reason = "unexpected_helper_record".into();
                        break;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout)
                if !ready && started.elapsed() >= Duration::from_secs(5) =>
            {
                reason = "ready_timeout".into();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                reason = "pipes_disconnected".into();
                break;
            }
        }
    }
    drop(child_stdin);
    let wait_until = Instant::now() + Duration::from_secs(1);
    let mut exit_code = None;
    while Instant::now() < wait_until {
        match child.try_wait() {
            Ok(Some(s)) => {
                exit_code = s.code();
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => break,
        }
    }
    if exit_code.is_none() {
        terminate(&mut child);
        final_ok = false;
        reason = "helper_cleanup_unconfirmed".into();
    }
    let expected = if reason == "requested" { 0 } else { 3 };
    if exit_code != Some(expected) {
        final_ok = false;
        if reason == "requested" || reason == "focus_changed" {
            reason = "helper_exit_mismatch".into();
        }
    }
    // A final line is not sufficient: reject additional helper output and require
    // stdout to close. Detached descendants may otherwise retain the pipe.
    if final_ok {
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut closed = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Message::Helper(Err(e))) if e == "helper_closed" => {
                    closed = true;
                    break;
                }
                Ok(Message::Helper(_)) => {
                    reason = "unexpected_trailing_output".into();
                    break;
                }
                Ok(Message::Control(_)) => (),
                Err(_) => break,
            }
        }
        if !closed {
            final_ok = false;
            if reason != "unexpected_trailing_output" {
                reason = "helper_output_not_closed".into();
            }
        }
    }
    let record = json!({"status":if final_ok {"stopped"} else {"error"},"reason":reason,"helper_exit_code":exit_code,
        "action_allowed":false,"input_posted":false,"input_isolation_verified":false,
        "counts":final_counts, "frontmost_same_at_end":frontmost_same_at_end, "foreground_identity":baseline_identity,
        "scope":if test_own {"own_child_test"} else {"frontmost_app"},
        "note":if test_own {"Only the helper's own headless child is observed. This does not authorize input."} else {"Only one initially foreground app is monitored. Notifications can be missed; this does not authorize input."}});
    write_record(&mut file, &record)?;
    file.sync_all().map_err(|_| "evidence_sync_failed")?;
    Ok(final_ok && record["reason"] == "requested")
}
fn main() {
    let args: Vec<String> = env::args().collect();
    let (test_own, helper, evidence) = match args.as_slice() {
        [_, helper_flag, helper, evidence_flag, evidence]
            if helper_flag == "--helper" && evidence_flag == "--evidence" =>
        {
            (false, helper, evidence)
        }
        [_, test_flag, helper_flag, helper, evidence_flag, evidence]
            if test_flag == "--self-test-notification"
                && helper_flag == "--helper"
                && evidence_flag == "--evidence" =>
        {
            (true, helper, evidence)
        }
        _ => {
            eprintln!("usage: cu-focus-watch [--self-test-notification] --helper /absolute/helper --evidence /absolute/new.jsonl");
            std::process::exit(2);
        }
    };
    match run(Path::new(helper), Path::new(evidence), test_own) {
        Ok(true) => (),
        Ok(false) => std::process::exit(3),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
