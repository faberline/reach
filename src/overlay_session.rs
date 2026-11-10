//! One bounded, display-only cursor overlay session.
//!
//! `run` starts the overlay helper, sends a fixed list of steps, and always
//! stops the helper before it returns. The overlay exists only during the
//! call. Nothing here sends input, moves the real cursor, or authorizes an
//! action. After any failure the overlay state is reported as unknown; an
//! earlier reply is never reused as the current state.

use crate::{
    cursor_overlay::{self, Command, Phase, Point, Request},
    virtual_input::{Intent, PlannedEvent},
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

pub const SCHEMA: &str = "cursor-overlay-session-v1";
pub const MAX_STEPS: usize = 64;
pub const MAX_WAIT_MS: u64 = 10_000;
pub const MIN_TIMEOUT_MS: u64 = 100;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
/// Longest time to wait for one reply line.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest time to wait for the helper to stop after `quit`.
pub const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_LINE: u64 = 8192;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Command(Command),
    Wait(Duration),
}

fn exact_keys(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == keys.len() && keys.iter().all(|k| m.contains_key(*k)))
}

fn step_point(value: &Value) -> Result<Point, String> {
    if !exact_keys(value, &["command", "display_id", "x", "y"]) {
        return Err("invalid step".into());
    }
    let display_id = value["display_id"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("invalid step")?;
    let coordinate = |key: &str| {
        value[key]
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= cursor_overlay::MAX_COORDINATE)
            .ok_or_else(|| "invalid step".to_string())
    };
    Ok(Point {
        display_id,
        x: coordinate("x")?,
        y: coordinate("y")?,
    })
}

/// Parses the `steps` array of an `overlay_session` request.
/// `quit` is not a step: `run` always sends it at the end.
pub fn parse_steps(value: &Value) -> Result<Vec<Step>, String> {
    let items = value
        .as_array()
        .filter(|a| (1..=MAX_STEPS).contains(&a.len()))
        .ok_or("invalid steps")?;
    let mut steps = Vec::with_capacity(items.len());
    for item in items {
        let bare = exact_keys(item, &["command"]);
        let step = match item["command"].as_str() {
            Some("show") => Step::Command(Command::Show(step_point(item)?)),
            Some("move") => Step::Command(Command::Move(step_point(item)?)),
            Some("phase") if exact_keys(item, &["command", "phase"]) => {
                Step::Command(Command::Phase(
                    item["phase"]
                        .as_str()
                        .and_then(Phase::parse)
                        .ok_or("invalid step")?,
                ))
            }
            Some("hide") if bare => Step::Command(Command::Hide),
            Some("query") if bare => Step::Command(Command::Query),
            Some("wait") if exact_keys(item, &["command", "ms"]) => Step::Wait(
                item["ms"]
                    .as_u64()
                    .filter(|ms| (1..=MAX_WAIT_MS).contains(ms))
                    .map(Duration::from_millis)
                    .ok_or("invalid step")?,
            ),
            _ => return Err("invalid step".into()),
        };
        steps.push(step);
    }
    Ok(steps)
}

/// Overlay commands that display a planned virtual-input sequence.
///
/// `origin` is the caller's unverified claim of the window's top-left corner
/// in global display points. The result is display data only: it contains no
/// deliverable event, and showing it does not authorize the planned input.
pub fn commands_for_planned(origin: Point, events: &[PlannedEvent]) -> Vec<Command> {
    let mut commands = Vec::new();
    let mut current = Phase::Idle;
    for (index, event) in events.iter().enumerate() {
        let point = Point {
            display_id: origin.display_id,
            x: origin.x + event.point_in_window_points.x,
            y: origin.y + event.point_in_window_points.y,
        };
        if index == 0 {
            commands.push(Command::Show(point));
        } else {
            commands.push(Command::Move(point));
        }
        let held = event.button_after.is_some() || event.keys_after != [0, 0];
        let phase = match (held, event.intent) {
            (false, _) => Phase::Idle,
            (true, Intent::Move) if event.button_after.is_some() => Phase::Drag,
            (true, _) => Phase::Hold,
        };
        if phase != current {
            commands.push(Command::Phase(phase));
            current = phase;
        }
    }
    commands
}

enum Line {
    Text(Vec<u8>),
    TooLong,
}

fn read_lines(stdout: impl Read + Send + 'static) -> Receiver<Line> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match (&mut reader)
                .take(MAX_LINE + 1)
                .read_until(b'\n', &mut line)
            {
                Ok(0) | Err(_) => return,
                Ok(_) if line.last() != Some(&b'\n') => {
                    // Either an oversized line or output that ended without a newline.
                    let _ = tx.send(Line::TooLong);
                    return;
                }
                Ok(_) => {
                    line.pop();
                    if tx.send(Line::Text(line)).is_err() {
                        return;
                    }
                }
            }
        }
    });
    rx
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Show(_) => "show",
        Command::Move(_) => "move",
        Command::Phase(_) => "phase",
        Command::Hide => "hide",
        Command::Query => "query",
        Command::Quit => "quit",
    }
}

fn send(stdin: &mut ChildStdin, id: &str, command: Command) -> bool {
    let request = Request {
        id: id.to_string(),
        command,
    };
    let mut line = cursor_overlay::request_json(&request).to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .and_then(|_| stdin.flush())
        .is_ok()
}

/// Waits for the reply to `id`. `Err` is `(reason, helper_reason)`.
fn receive(
    rx: &Receiver<Line>,
    id: &str,
    limit: Duration,
) -> Result<cursor_overlay::Reply, (&'static str, Option<String>)> {
    match rx.recv_timeout(limit) {
        Err(RecvTimeoutError::Timeout) => Err(("timeout", None)),
        Err(RecvTimeoutError::Disconnected) => Err(("helper_exited", None)),
        Ok(Line::TooLong) => Err(("invalid_reply", None)),
        Ok(Line::Text(bytes)) => match cursor_overlay::parse_reply(&bytes) {
            Err(_) => Err(("invalid_reply", None)),
            Ok(reply) if reply.id.as_deref() != Some(id) => {
                Err(("unsolicited_reply", reply.reason))
            }
            Ok(reply) if reply.reason.is_some() => Err(("helper_refused", reply.reason)),
            Ok(reply) => Ok(reply),
        },
    }
}

/// Sends `quit`, then makes sure the helper process has ended.
/// Returns `(cleanup, overlay_after)`.
fn stop_helper(
    child: &mut Child,
    stdin: Option<ChildStdin>,
    rx: &Receiver<Line>,
    ask: bool,
) -> (&'static str, &'static str) {
    let mut acknowledged = false;
    if let Some(mut stdin) = stdin {
        if ask && send(&mut stdin, "quit", Command::Quit) {
            acknowledged = receive(rx, "quit", CLEANUP_TIMEOUT).is_ok_and(|r| !r.state.visible);
        }
        // Dropping stdin closes the pipe; the helper also hides and exits on end of input.
    }
    if acknowledged {
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return ("quit_acknowledged", "cleared"),
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
    }
    // A killed process cannot keep a window on screen.
    let _ = child.kill();
    match child.wait() {
        Ok(_) => ("killed", "cleared"),
        Err(_) => ("unconfirmed", "unknown"),
    }
}

fn report(
    steps: Vec<Value>,
    stop: Value,
    state: Value,
    cleanup: &str,
    overlay_after: &str,
) -> Value {
    json!({"schema": SCHEMA, "action_allowed": false,
        "window_binding": cursor_overlay::WINDOW_BINDING,
        "completed": stop.is_null(), "stop": stop, "steps": steps,
        "last_reported_state": state, "cleanup": cleanup, "overlay_after": overlay_after,
        "note": "display only; no input was sent and nothing here authorizes input"})
}

/// Runs one overlay session. `timeout` bounds all steps together.
/// The helper path is a trusted caller input.
pub fn run(helper: &Path, steps: &[Step], timeout: Duration) -> Value {
    let stopped = |index: Value, reason: &str, helper_reason: Option<String>| json!({"step": index, "reason": reason, "helper_reason": helper_reason});
    let spawned = std::process::Command::new(helper)
        .arg("--serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(_) => {
            let stop = stopped(Value::Null, "helper_unavailable", None);
            return report(
                Vec::new(),
                stop,
                json!("unknown"),
                "not_started",
                "not_shown",
            );
        }
    };
    let (mut stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => (stdin, stdout),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            let stop = stopped(Value::Null, "helper_unavailable", None);
            return report(Vec::new(), stop, json!("unknown"), "killed", "cleared");
        }
    };
    let rx = read_lines(stdout);
    let deadline = Instant::now() + timeout;
    let mut done = Vec::with_capacity(steps.len());
    let mut stop = Value::Null;
    let mut state = json!("unknown");
    for (index, step) in steps.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let failure = match *step {
            Step::Wait(wait) => match rx.recv_timeout(wait.min(remaining)) {
                Err(RecvTimeoutError::Timeout) if wait <= remaining => {
                    done.push(json!({"index": index, "command": "wait",
                        "ms": wait.as_millis() as u64}));
                    None
                }
                Err(RecvTimeoutError::Timeout) => Some(("timeout", None)),
                Err(RecvTimeoutError::Disconnected) => Some(("helper_exited", None)),
                Ok(Line::TooLong) => Some(("invalid_reply", None)),
                Ok(Line::Text(bytes)) => match cursor_overlay::parse_reply(&bytes) {
                    Ok(reply) => Some(("unsolicited_reply", reply.reason)),
                    Err(_) => Some(("invalid_reply", None)),
                },
            },
            Step::Command(command) => {
                let id = format!("s{index}");
                if !send(&mut stdin, &id, command) {
                    Some(("helper_exited", None))
                } else {
                    match receive(&rx, &id, REPLY_TIMEOUT.min(remaining)) {
                        Ok(reply) => {
                            state = json!({"visible": reply.state.visible,
                                "phase": reply.state.phase.as_str(),
                                "point": reply.state.point.map(|p| json!({
                                    "space": cursor_overlay::SPACE,
                                    "display_id": p.display_id, "x": p.x, "y": p.y}))});
                            done.push(json!({"index": index,
                                "command": command_name(&command), "state": state}));
                            None
                        }
                        Err(failure) => Some(failure),
                    }
                }
            }
        };
        if let Some((reason, helper_reason)) = failure {
            stop = stopped(json!(index), reason, helper_reason);
            // A state reported before the failure is not the current state.
            state = json!("unknown");
            break;
        }
    }
    let ask = stop.is_null() || stop["reason"] == "helper_refused";
    let (cleanup, overlay_after) = stop_helper(&mut child, Some(stdin), &rx, ask);
    report(done, stop, state, cleanup, overlay_after)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{input_guard::WindowPoint, virtual_input::Button, Target};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    /// A fake helper: a POSIX shell script. `body` runs once per request line with `$id` set.
    fn helper(body: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "overlay-session-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let script = format!(
            "#!/bin/sh\nreply() {{ printf '{{\"protocol\":\"cursor-overlay-v1\",\"id\":%s,\"status\":\"%s\",\"reason\":%s,\"visible\":%s,\"phase\":\"%s\",\"point\":%s,\"window_binding\":\"unbound\",\"action_allowed\":false}}\\n' \"$1\" \"$2\" \"$3\" \"$4\" \"$5\" \"$6\"; }}\nP='{{\"space\":\"global_display_points_top_left\",\"display_id\":1,\"x\":10,\"y\":20}}'\nwhile IFS= read -r line; do\n  id=$(printf '%s' \"$line\" | sed 's/.*\"id\":\"\\([^\"]*\\)\".*/\\1/')\n  {body}\ndone\n"
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    const SHOWN: &str = r#"reply "\"$id\"" ok null true idle "$P""#;
    /// Replies like the real helper for `quit`, and shown for everything else.
    const POLITE: &str = r#"case "$line" in *'"quit"'*) reply "\"$id\"" ok null false idle null; exit 0;; *) reply "\"$id\"" ok null true idle "$P";; esac"#;

    fn show() -> Step {
        Step::Command(Command::Show(Point {
            display_id: 1,
            x: 10.0,
            y: 20.0,
        }))
    }
    fn query() -> Step {
        Step::Command(Command::Query)
    }
    fn run_with(body: &str, steps: &[Step], timeout_ms: u64) -> Value {
        let path = helper(body);
        let result = run(&path, steps, Duration::from_millis(timeout_ms));
        let _ = fs::remove_file(path);
        result
    }

    #[test]
    fn parses_every_step_kind() {
        let steps = parse_steps(&json!([
            {"command":"show","display_id":1,"x":10,"y":20.5},
            {"command":"move","display_id":2,"x":-1,"y":0},
            {"command":"phase","phase":"hold"},
            {"command":"wait","ms":250},
            {"command":"query"},
            {"command":"hide"}
        ]))
        .unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Command(Command::Show(Point {
                    display_id: 1,
                    x: 10.0,
                    y: 20.5
                })),
                Step::Command(Command::Move(Point {
                    display_id: 2,
                    x: -1.0,
                    y: 0.0
                })),
                Step::Command(Command::Phase(Phase::Hold)),
                Step::Wait(Duration::from_millis(250)),
                Step::Command(Command::Query),
                Step::Command(Command::Hide),
            ]
        );
    }

    #[test]
    fn refuses_invalid_steps() {
        let too_many = Value::Array(vec![json!({"command":"query"}); MAX_STEPS + 1]);
        for bad in [
            json!([]),
            json!({"command":"query"}),
            too_many,
            json!([{"command":"quit"}]),
            json!([{"command":"click"}]),
            json!([{"command":"query","extra":1}]),
            json!([{"command":"hide","ms":1}]),
            json!([{"command":"show","display_id":0,"x":1,"y":1}]),
            json!([{"command":"show","display_id":1,"x":1}]),
            json!([{"command":"show","display_id":1,"x":1000001,"y":1}]),
            json!([{"command":"move","display_id":1.5,"x":1,"y":1}]),
            json!([{"command":"phase","phase":"press"}]),
            json!([{"command":"wait","ms":0}]),
            json!([{"command":"wait","ms":10001}]),
            json!([{"command":"wait","ms":"5"}]),
        ] {
            assert!(parse_steps(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn completed_session_reports_states_and_clears() {
        let steps = [show(), Step::Wait(Duration::from_millis(20)), query()];
        let r = run_with(POLITE, &steps, 30_000);
        assert_eq!(r["schema"], SCHEMA);
        assert_eq!(r["action_allowed"], false);
        assert_eq!(r["window_binding"], "unbound");
        assert_eq!(r["completed"], true);
        assert_eq!(r["stop"], Value::Null);
        assert_eq!(r["steps"].as_array().unwrap().len(), 3);
        assert_eq!(r["steps"][0]["command"], "show");
        assert_eq!(r["steps"][0]["state"]["visible"], true);
        assert_eq!(r["steps"][0]["state"]["point"]["x"], 10.0);
        assert_eq!(r["steps"][1], json!({"index":1,"command":"wait","ms":20}));
        assert_eq!(r["last_reported_state"]["visible"], true);
        assert_eq!(r["cleanup"], "quit_acknowledged");
        assert_eq!(r["overlay_after"], "cleared");
    }

    #[test]
    fn missing_helper_is_reported_without_a_state() {
        let r = run(
            Path::new("/nonexistent/overlay-helper"),
            &[show()],
            Duration::from_secs(1),
        );
        assert_eq!(r["completed"], false);
        assert_eq!(r["stop"]["reason"], "helper_unavailable");
        assert_eq!(r["last_reported_state"], "unknown");
        assert_eq!(r["overlay_after"], "not_shown");
        assert_eq!(r["action_allowed"], false);
    }

    #[test]
    fn helper_exit_makes_the_state_unknown() {
        // Answers the first request, then exits before the second.
        let body = format!("{SHOWN}; exit 0");
        let r = run_with(&body, &[show(), query()], 30_000);
        assert_eq!(r["completed"], false);
        assert_eq!(r["stop"]["step"], 1);
        assert_eq!(r["stop"]["reason"], "helper_exited");
        assert_eq!(r["steps"].as_array().unwrap().len(), 1);
        assert_eq!(r["last_reported_state"], "unknown");
        assert_eq!(r["overlay_after"], "cleared");
    }

    #[test]
    fn silent_helper_times_out_and_is_killed() {
        let r = run_with(":", &[show(), query()], 300);
        assert_eq!(r["completed"], false);
        assert_eq!(r["stop"]["step"], 0);
        assert_eq!(r["stop"]["reason"], "timeout");
        assert_eq!(r["last_reported_state"], "unknown");
        assert_eq!(r["cleanup"], "killed");
        assert_eq!(r["overlay_after"], "cleared");
    }

    #[test]
    fn session_deadline_stops_a_long_wait() {
        let steps = [show(), Step::Wait(Duration::from_millis(10_000)), query()];
        let r = run_with(POLITE, &steps, 4000);
        assert_eq!(r["completed"], false);
        assert_eq!(r["stop"]["step"], 1);
        assert_eq!(r["stop"]["reason"], "timeout");
        assert_eq!(r["last_reported_state"], "unknown");
        assert_eq!(r["overlay_after"], "cleared");
    }

    #[test]
    fn invalid_reply_stops_the_session() {
        for body in [
            "printf 'not json\\n'",
            // `action_allowed` must be false.
            r#"printf '{"protocol":"cursor-overlay-v1","id":"%s","status":"ok","reason":null,"visible":false,"phase":"idle","point":null,"window_binding":"unbound","action_allowed":true}\n' "$id""#,
        ] {
            let r = run_with(body, &[show(), query()], 30_000);
            assert_eq!(r["completed"], false, "{body}");
            assert_eq!(r["stop"]["reason"], "invalid_reply", "{body}");
            assert_eq!(r["last_reported_state"], "unknown", "{body}");
            assert_eq!(r["cleanup"], "killed", "{body}");
        }
    }

    #[test]
    fn refusal_and_display_change_stop_the_session() {
        let refused = r#"reply "\"$id\"" error '"unknown_display"' false idle null"#;
        let r = run_with(refused, &[show(), query()], 30_000);
        assert_eq!(r["stop"]["reason"], "helper_refused");
        assert_eq!(r["stop"]["helper_reason"], "unknown_display");
        assert_eq!(r["last_reported_state"], "unknown");

        // The helper reports a display change with a null id instead of answering.
        let changed = r#"reply null error '"display_changed"' false idle null"#;
        let r = run_with(changed, &[show(), query()], 30_000);
        assert_eq!(r["completed"], false);
        assert_eq!(r["stop"]["step"], 0);
        assert_eq!(r["stop"]["reason"], "unsolicited_reply");
        assert_eq!(r["stop"]["helper_reason"], "display_changed");
        assert_eq!(r["last_reported_state"], "unknown");
        assert_eq!(r["overlay_after"], "cleared");
    }

    #[test]
    fn planned_events_become_display_commands_only() {
        let target = Target::new(1, 1).unwrap();
        let event = |sequence, x, intent, button_after, keys_after| PlannedEvent {
            sequence,
            target,
            point_in_window_points: WindowPoint { x, y: 5.0 },
            intent,
            button_before: None,
            button_after,
            keys_before: [0, 0],
            keys_after,
        };
        let left = Some(Button::Left);
        let events = [
            event(1, 1.0, Intent::Move, None, [0, 0]),
            event(2, 1.0, Intent::ButtonDown(Button::Left), left, [0, 0]),
            event(3, 2.0, Intent::Move, left, [0, 0]),
            event(4, 2.0, Intent::ButtonUp(Button::Left), None, [0, 0]),
            event(5, 2.0, Intent::KeyDown(0), None, [1, 0]),
        ];
        let origin = Point {
            display_id: 3,
            x: 100.0,
            y: 200.0,
        };
        let at = |x| Point {
            display_id: 3,
            x,
            y: 205.0,
        };
        assert_eq!(
            commands_for_planned(origin, &events),
            vec![
                Command::Show(at(101.0)),
                Command::Move(at(101.0)),
                Command::Phase(Phase::Hold),
                Command::Move(at(102.0)),
                Command::Phase(Phase::Drag),
                Command::Move(at(102.0)),
                Command::Phase(Phase::Idle),
                Command::Move(at(102.0)),
                Command::Phase(Phase::Hold),
            ]
        );
        assert!(commands_for_planned(origin, &[]).is_empty());
        assert!(events.iter().all(|e| !e.action_allowed()));
    }
}
