//! Client of the ReachAgent socket. Unix only.
//!
//! Sends one request line and reads one reply line per connection, using the protocol in
//! `agent_protocol`. It never starts the App, never asks for a permission and never sends
//! an input event: the only methods it uses are `describe`, `permissions`, `capture`,
//! `capture_all_windows`, `accessibility`, `accessibility_subtree` and the read-only
//! observations `input_source`, `secure_input`, `pointer_location` and `bundle_windows`.
use crate::agent_protocol::{
    check_accessibility_result, check_capture_result, check_observation_result,
    check_subtree_result, parse_accessibility_params, parse_bundle_windows_params,
    parse_capture_params, parse_subtree_params, Method, MAX_LINE_BYTES, PROTOCOL_VERSION,
};
use serde_json::{json, Map, Value};
use std::{
    io::{ErrorKind, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const SOCKET_ENV: &str = "REACH_AGENT_SOCKET";
/// Longest socket path, in bytes (same limit as the service).
pub const MAX_SOCKET_PATH_BYTES: usize = 100;
/// Read timeout of a routed capture or accessibility call (the service gives up after 30 s).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(35);
/// Read timeout of the `describe` status probe.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// errno of "the path is not a socket" (no libc dependency).
#[cfg(target_os = "linux")]
const ENOTSOCK: i32 = 88;
#[cfg(not(target_os = "linux"))]
const ENOTSOCK: i32 = 38;

/// `env_socket` is `$REACH_AGENT_SOCKET`, `home` is `$HOME`.
pub fn socket_path(env_socket: Option<&str>, home: Option<&str>) -> Result<PathBuf, String> {
    let path = match (env_socket, home) {
        (Some(path), _) => path.to_string(),
        (None, Some(home)) if home.starts_with('/') => format!("{home}/.reach/agent.sock"),
        (None, Some(_)) => return Err("HOME must be an absolute path".to_string()),
        (None, None) => {
            return Err(format!(
                "neither {SOCKET_ENV} nor HOME is set, so there is no socket path"
            ))
        }
    };
    if !path.starts_with('/') || path.contains('\0') {
        return Err(format!(
            "the socket path must be absolute and contain no NUL (from {SOCKET_ENV} or HOME)"
        ));
    }
    if path.len() > MAX_SOCKET_PATH_BYTES {
        return Err(format!(
            "the socket path is longer than {MAX_SOCKET_PATH_BYTES} bytes"
        ));
    }
    Ok(PathBuf::from(path))
}

/// `socket_path` over the real environment.
pub fn default_socket_path() -> Result<PathBuf, String> {
    let env_socket = std::env::var(SOCKET_ENV).ok();
    let home = std::env::var("HOME").ok();
    socket_path(env_socket.as_deref(), home.as_deref())
}

/// How a person starts the App on this socket. Reach never runs it.
pub fn start_hint(socket: &Path) -> String {
    format!(
        "open -a ReachAgent --args --serve --socket {}",
        socket.display()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFailure {
    pub code: String,
    pub message: String,
    pub permission: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// Nothing is listening: the socket is missing, refused the connection, or is not a socket.
    Unavailable(String),
    /// The service answered `ok:false`.
    Agent(AgentFailure),
    /// Timeout, I/O error, or a reply that breaks the protocol.
    Protocol(String),
}

impl CallError {
    fn describe(&self) -> String {
        match self {
            CallError::Unavailable(reason) | CallError::Protocol(reason) => reason.clone(),
            CallError::Agent(f) => format!("{}: {}", f.code, f.message),
        }
    }
}

fn protocol(reason: impl Into<String>) -> CallError {
    CallError::Protocol(reason.into())
}

fn io_failure(what: &str, e: &std::io::Error) -> CallError {
    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
        protocol(format!("{what}: timed out"))
    } else {
        protocol(format!("{what}: {e}"))
    }
}

/// One request line out, one reply line in, on a fresh connection.
pub fn call(
    socket: &Path,
    id: &str,
    method: &str,
    params: &Value,
    timeout: Duration,
) -> Result<Value, CallError> {
    let deadline = Instant::now() + timeout;
    let mut stream = UnixStream::connect(socket).map_err(|e| {
        let reason = format!("cannot connect to the ReachAgent socket: {e}");
        if matches!(
            e.kind(),
            ErrorKind::NotFound | ErrorKind::ConnectionRefused | ErrorKind::NotConnected
        ) || e.raw_os_error() == Some(ENOTSOCK)
        {
            CallError::Unavailable(reason)
        } else {
            protocol(reason)
        }
    })?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| io_failure("socket timeout", &e))?;
    let mut line = json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|e| io_failure("write request", &e))?;
    // One deadline for the whole reply, not one per read.
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    let newline = loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| protocol("read reply: timed out"))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| io_failure("socket timeout", &e))?;
        let n = match stream.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_failure("read reply", &e)),
        };
        if n == 0 {
            return Err(protocol(
                "the service closed the connection without a complete reply line",
            ));
        }
        reply.extend_from_slice(&chunk[..n]);
        if let Some(i) = reply.iter().position(|b| *b == b'\n') {
            break i;
        }
        if reply.len() > MAX_LINE_BYTES {
            return Err(protocol("the reply line is too long"));
        }
    };
    reply.truncate(newline);
    if reply.len() > MAX_LINE_BYTES {
        return Err(protocol("the reply line is too long"));
    }
    parse_reply(id, &reply)
}

fn parse_reply(id: &str, reply: &[u8]) -> Result<Value, CallError> {
    let value = crate::strict_json::from_slice(reply)
        .map_err(|e| protocol(format!("the reply is not valid JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| protocol("the reply is not a JSON object"))?;
    if object.get("version").and_then(Value::as_u64) != Some(u64::from(PROTOCOL_VERSION)) {
        return Err(protocol("the reply has the wrong protocol version"));
    }
    if object.get("id").and_then(Value::as_str) != Some(id) {
        return Err(protocol("the reply id differs from the request id"));
    }
    match object.get("ok") {
        Some(Value::Bool(true)) => match object.get("result") {
            Some(result) if result.is_object() => Ok(result.clone()),
            _ => Err(protocol("an ok reply needs an object `result`")),
        },
        Some(Value::Bool(false)) => {
            let error = object
                .get("error")
                .and_then(Value::as_object)
                .ok_or_else(|| protocol("an error reply needs an object `error`"))?;
            let text = |key: &str| error.get(key).and_then(Value::as_str).map(str::to_string);
            match (text("code"), text("message")) {
                (Some(code), Some(message)) => Err(CallError::Agent(AgentFailure {
                    code,
                    message,
                    permission: text("permission"),
                    state: text("state"),
                })),
                _ => Err(protocol("an error reply needs string `code` and `message`")),
            }
        }
        _ => Err(protocol("the reply needs a boolean `ok`")),
    }
}

/// True for the four helper-backed methods when the request has neither `capture_helper` nor
/// `ax_helper` (a request with either key keeps the direct helper path), and always for the
/// four read-only observations, which only ReachAgent serves.
pub fn is_routed(request: &Value) -> bool {
    let Some(object) = request.as_object() else {
        return false;
    };
    matches!(
        object.get("method").and_then(Value::as_str),
        Some("capture" | "capture_all_windows" | "accessibility" | "accessibility_subtree")
    ) && !object.contains_key("capture_helper")
        && !object.contains_key("ax_helper")
        || matches!(
            object.get("method").and_then(Value::as_str),
            Some("input_source" | "secure_input" | "pointer_location" | "bundle_windows")
        )
}

#[derive(Debug, Clone, PartialEq)]
pub enum Routed {
    Done {
        status: &'static str,
        result: Value,
    },
    Unavailable {
        reason: String,
    },
    Failed {
        reason: String,
        agent_error: Option<Value>,
    },
}

fn failed(reason: impl Into<String>) -> Routed {
    Routed::Failed {
        reason: reason.into(),
        agent_error: None,
    }
}

type Checker = Box<dyn Fn(&Value) -> Result<(), String>>;

/// Validate, call, check. `request` is the whole `cu-agent` request (`id`, `method`, fields).
pub fn route(request: &Value, socket: &Path) -> Routed {
    let Some(object) = request.as_object() else {
        return failed("request must be an object");
    };
    let (Some(id), Some(method_name)) = (
        object.get("id").and_then(Value::as_str),
        object.get("method").and_then(Value::as_str),
    ) else {
        return failed("request needs a string id and method");
    };
    let (required, optional): (&[&str], &[&str]) = match Method::parse(method_name) {
        Some(Method::Capture | Method::CaptureAllWindows) => (&["pid", "window_id", "image"], &[]),
        Some(Method::Accessibility) => (&["pid"], &["windows_only"]),
        Some(Method::AccessibilitySubtree) => (&["pid", "source", "path"], &[]),
        Some(Method::InputSource | Method::SecureInput | Method::PointerLocation) => (&[], &[]),
        Some(Method::BundleWindows) => (&["bundle_id"], &[]),
        _ => return failed("method is not routed through ReachAgent"),
    };
    let method = Method::parse(method_name).expect("parsed above");
    if required.iter().any(|k| !object.contains_key(*k))
        || object.keys().any(|k| {
            !["id", "method"].contains(&k.as_str())
                && !required.contains(&k.as_str())
                && !optional.contains(&k.as_str())
        })
    {
        return failed("missing or unexpected request fields");
    }
    let params: Map<String, Value> = object
        .iter()
        .filter(|(k, _)| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let params = Value::Object(params);
    let checker: Checker = match method {
        Method::Capture | Method::CaptureAllWindows => match parse_capture_params(&params) {
            Ok(p) => Box::new(move |r| check_capture_result(method, &p, r)),
            Err(e) => return failed(e.message),
        },
        Method::Accessibility => match parse_accessibility_params(&params) {
            Ok(p) => Box::new(move |r| check_accessibility_result(&p, r)),
            Err(e) => return failed(e.message),
        },
        Method::AccessibilitySubtree => match parse_subtree_params(&params) {
            Ok(p) => Box::new(move |r| check_subtree_result(&p, r)),
            Err(e) => return failed(e.message),
        },
        Method::BundleWindows => match parse_bundle_windows_params(&params) {
            Ok(bundle_id) => {
                Box::new(move |r| check_observation_result(method, Some(&bundle_id), r))
            }
            Err(e) => return failed(e.message),
        },
        _ => Box::new(move |r| check_observation_result(method, None, r)),
    };
    let result = match call(socket, id, method_name, &params, CALL_TIMEOUT) {
        Ok(result) => result,
        Err(CallError::Unavailable(reason)) => return Routed::Unavailable { reason },
        Err(CallError::Agent(f)) => {
            let mut error = json!({"code": f.code, "message": f.message});
            if let Some(permission) = &f.permission {
                error["permission"] = json!(permission);
            }
            if let Some(state) = &f.state {
                error["state"] = json!(state);
            }
            return Routed::Failed {
                reason: format!("{}: {}", f.code, f.message),
                agent_error: Some(error),
            };
        }
        Err(CallError::Protocol(reason)) => return failed(reason),
    };
    if let Err(mismatch) = checker(&result) {
        return failed(format!("agent result mismatch: {mismatch}"));
    }
    // A read-only observation answers its own question; a capture or subtree candidate still
    // needs the caller to confirm it.
    let status = if matches!(
        method,
        Method::Accessibility
            | Method::InputSource
            | Method::SecureInput
            | Method::PointerLocation
            | Method::BundleWindows
    ) {
        "ok"
    } else {
        "unknown"
    };
    Routed::Done { status, result }
}

fn status_failure(socket: &Path, error: &CallError) -> Value {
    let shown = socket.display().to_string();
    match error {
        CallError::Unavailable(reason) => json!({
            "state": "unavailable", "socket": shown, "reason": reason,
            "start": start_hint(socket),
        }),
        other => json!({"state": "error", "socket": shown, "reason": other.describe()}),
    }
}

/// The `input` block of ReachAgent's `describe`, checked and copied key by key.
fn input_block(block: &Value) -> Option<Value> {
    let o = block.as_object()?;
    let state = o
        .get("state")?
        .as_str()
        .filter(|s| ["armed", "disabled"].contains(s))?;
    let backend = o
        .get("backend")?
        .as_str()
        .filter(|s| ["fake", "system"].contains(s))?;
    if o.len() == 2 {
        return (state == "disabled").then(|| json!({"state": state, "backend": backend}));
    }
    let bundles = o.get("allowed_bundle_ids")?.as_array()?;
    if !bundles.iter().all(|b| {
        b.as_str()
            .is_some_and(|b| crate::foreground_input::APPROVED_BUNDLE_IDS.contains(&b))
    }) {
        return None;
    }
    let keycodes = o.get("allowed_keycodes")?.as_array()?;
    if !keycodes.iter().all(|k| {
        k.as_u64()
            .and_then(|k| u16::try_from(k).ok())
            .is_some_and(|k| crate::foreground_input::APPROVED_KEYCODES.contains(&k))
    }) {
        return None;
    }
    let expires = o.get("expires_unix_seconds")?.as_f64()?;
    let remaining = o.get("events_remaining")?.as_u64()?;
    let halted = o.get("halted")?.as_bool()?;
    if o.get("verified") != Some(&Value::Bool(false)) || o.len() != 8 {
        return None;
    }
    Some(json!({
        "state": state, "backend": backend, "allowed_bundle_ids": bundles,
        "allowed_keycodes": keycodes, "expires_unix_seconds": expires,
        "events_remaining": remaining, "halted": halted, "verified": false,
    }))
}

/// For `describe`: the App's state, read with `describe` and `permissions` only.
/// `socket` is `Err(reason)` when no socket path could be decided. Never fails.
pub fn app_status(socket: Result<&Path, &str>) -> Value {
    let socket = match socket {
        Ok(path) => path,
        Err(reason) => return json!({"state": "unavailable", "reason": reason}),
    };
    let probe = |method: &str| call(socket, "status", method, &json!({}), STATUS_TIMEOUT);
    let described = match probe("describe") {
        Ok(v) => v,
        Err(e) => return status_failure(socket, &e),
    };
    let permissions = match probe("permissions") {
        Ok(v) => v,
        Err(e) => return status_failure(socket, &e),
    };
    let state = |key: &str| {
        permissions
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| ["granted", "denied", "unknown"].contains(s))
    };
    let (Some(app_version), Some(protocol_version), Some(test_mode)) = (
        described.get("app_version").and_then(Value::as_str),
        described.get("protocol_version").and_then(Value::as_u64),
        described.get("test_mode").and_then(Value::as_bool),
    ) else {
        return status_failure(socket, &protocol("describe reply has unexpected fields"));
    };
    let (Some(accessibility), Some(screen_recording), Some(post_event)) = (
        state("accessibility"),
        state("screen_recording"),
        state("post_event"),
    ) else {
        return status_failure(socket, &protocol("permissions reply has unexpected fields"));
    };
    // The `input` block exists only when ReachAgent started with `--enable-input`.
    let input = match described.get("input") {
        None => json!({"state": "disabled"}),
        Some(block) => match input_block(block) {
            Some(block) => block,
            None => return status_failure(socket, &protocol("describe input block is malformed")),
        },
    };
    json!({
        "state": "running",
        "socket": socket.display().to_string(),
        "app_version": app_version,
        "protocol_version": protocol_version,
        "test_mode": test_mode,
        "permissions": {
            "accessibility": accessibility,
            "screen_recording": screen_recording,
            "post_event": post_event,
        },
        "input": input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_protocol::parse_request;
    use std::{
        fs,
        io::{BufRead, BufReader},
        os::unix::net::UnixListener,
        sync::atomic::{AtomicU64, Ordering},
        thread::JoinHandle,
        time::Instant,
    };

    enum Reply {
        Line(Value),
        Raw(Vec<u8>),
        Close,
        Hang,
    }

    /// A fake App: accepts one connection per reply, in order, and records each request line.
    struct FakeApp {
        dir: PathBuf,
        socket: PathBuf,
        thread: Option<JoinHandle<Vec<String>>>,
    }

    fn temp_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ra{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    impl FakeApp {
        fn idle() -> FakeApp {
            let dir = temp_dir();
            FakeApp {
                socket: dir.join("s.sock"),
                dir,
                thread: None,
            }
        }
        fn spawn(replies: Vec<Reply>) -> FakeApp {
            let mut app = FakeApp::idle();
            let listener = UnixListener::bind(&app.socket).unwrap();
            app.thread = Some(std::thread::spawn(move || {
                let mut seen = Vec::new();
                for reply in replies {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut line = String::new();
                    BufReader::new(&stream).read_line(&mut line).unwrap();
                    seen.push(line.trim_end().to_string());
                    match reply {
                        Reply::Line(v) => {
                            let _ = stream.write_all(format!("{v}\n").as_bytes());
                        }
                        Reply::Raw(bytes) => {
                            let _ = stream.write_all(&bytes);
                        }
                        Reply::Close => {}
                        Reply::Hang => std::thread::sleep(Duration::from_millis(600)),
                    }
                }
                seen
            }));
            app
        }
        /// The request lines received, after every reply was served.
        fn finish(mut self) -> Vec<String> {
            self.thread.take().unwrap().join().unwrap()
        }
    }

    impl Drop for FakeApp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.socket);
            let _ = fs::remove_dir(&self.dir);
        }
    }

    fn ok(id: &str, result: Value) -> Reply {
        Reply::Line(json!({"version": 1, "id": id, "ok": true, "result": result}))
    }

    fn capture_report(pid: i64, window_id: u64, image: &str, scope: &str) -> Value {
        json!({
            "window_id": window_id,
            "owner_pid": pid,
            "window_frame_points": {"x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0},
            "filter_content_rect_points": {"x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0},
            "point_pixel_scale": 1.0,
            "image_width": 2,
            "image_height": 2,
            "capture_start_uptime_ns": 1,
            "capture_end_uptime_ns": 2,
            "capture_end_unix_seconds": 0.0,
            "image_path": image,
            "capture_type": "desktop_independent_window",
            "window_scope": scope,
            "onscreen_before": true,
            "onscreen_after": true,
            "occlusion_checked": false
        })
    }

    fn accessibility_report(tree_scope: &str) -> Value {
        json!({
            "schema_version": 1, "status": "ok", "pid": 42,
            "process_start_unix_seconds": 1.0,
            "process_start_source": "kernel_sysctl_kern_proc",
            "bundle_id": null, "executable": null,
            "scope": "process_ax_windows", "tree_scope": tree_scope,
            "cg_window_binding": "unverified", "window_count_reported": 1,
            "node_count": 1, "truncated": false, "action_allowed": false,
            "windows": [{
                "path": [0], "index": 0, "owner_pid": 42, "role": "AXWindow",
                "title": "Fake", "actions": [],
                "frame": {"x": 0, "y": 0, "width": 2, "height": 2}, "children": []
            }],
            "fallback": null
        })
    }

    fn subtree_report() -> Value {
        json!({
            "schema_version": 1, "status": "ok", "pid": 42,
            "process_start_unix_seconds": 1.0,
            "process_start_source": "kernel_sysctl_kern_proc",
            "source": "main", "path": [0, 1],
            "node": {"path": [0, 1], "owner_pid": 42, "role": "AXGroup", "title": null,
                     "actions": [], "frame": null, "children": []},
            "node_count": 1, "truncated": false,
            "scope": "read_only_subtree_unverified",
            "cg_window_binding": "unverified", "action_allowed": false
        })
    }

    fn capture_request(method: &str) -> Value {
        json!({"id": "c1", "method": method, "pid": 42, "window_id": 7, "image": "/tmp/a.png"})
    }

    #[test]
    fn socket_path_prefers_env_then_home() {
        let path = socket_path;
        assert_eq!(
            path(Some("/a/b.sock"), Some("/h")),
            Ok(PathBuf::from("/a/b.sock"))
        );
        assert_eq!(
            path(None, Some("/h")),
            Ok(PathBuf::from("/h/.reach/agent.sock"))
        );
        assert!(
            path(Some("rel"), Some("/h")).is_err(),
            "no fallback to HOME"
        );
        assert!(path(None, None).is_err());
        assert!(path(None, Some("rel")).is_err());
        let too_long = format!("/{}", "a".repeat(100));
        let longest = format!("/{}", "a".repeat(99));
        assert!(path(Some(&too_long), None).is_err());
        assert!(path(Some(&longest), None).is_ok());
        assert!(path(Some("/a\0b"), None).is_err());
    }

    #[test]
    fn start_hint_names_the_app_and_the_socket() {
        assert_eq!(
            start_hint(Path::new("/x/s.sock")),
            "open -a ReachAgent --args --serve --socket /x/s.sock"
        );
    }

    #[test]
    fn call_sends_one_request_line_and_returns_the_result() {
        let app = FakeApp::spawn(vec![ok("x", json!({"k": 1}))]);
        let result = call(&app.socket, "x", "permissions", &json!({}), CALL_TIMEOUT);
        assert_eq!(result, Ok(json!({"k": 1})));
        let seen = app.finish();
        assert_eq!(seen.len(), 1);
        let request = parse_request(&seen[0]).expect("a valid protocol request");
        assert_eq!(request.id, "x");
        assert_eq!(request.method, Method::Permissions);
        assert_eq!(request.params, json!({}));
        let raw: Value = serde_json::from_str(&seen[0]).unwrap();
        assert_eq!(
            raw,
            json!({"version": 1, "id": "x", "method": "permissions", "params": {}})
        );
    }

    #[test]
    fn call_reports_a_missing_or_dead_socket_as_unavailable() {
        let dir = FakeApp::idle();
        let missing = dir.socket.clone();
        assert!(matches!(
            call(&missing, "x", "describe", &json!({}), CALL_TIMEOUT),
            Err(CallError::Unavailable(_))
        ));
        drop(UnixListener::bind(&dir.socket).unwrap());
        assert!(dir.socket.exists(), "the stale socket file stays");
        assert!(matches!(
            call(&dir.socket, "x", "describe", &json!({}), CALL_TIMEOUT),
            Err(CallError::Unavailable(_))
        ));
        let plain = dir.dir.join("plain");
        fs::write(&plain, b"not a socket").unwrap();
        let result = call(&plain, "x", "describe", &json!({}), CALL_TIMEOUT);
        let _ = fs::remove_file(&plain);
        assert!(
            matches!(result, Err(CallError::Unavailable(_))),
            "{result:?}"
        );
    }

    #[test]
    fn call_maps_service_errors() {
        let app = FakeApp::spawn(vec![Reply::Line(json!({
            "version": 1, "id": "x", "ok": false,
            "error": {"code": "permission_denied", "message": "no",
                      "permission": "screen_recording", "state": "denied"}
        }))]);
        let result = call(&app.socket, "x", "capture", &json!({}), CALL_TIMEOUT);
        assert_eq!(
            result,
            Err(CallError::Agent(AgentFailure {
                code: "permission_denied".into(),
                message: "no".into(),
                permission: Some("screen_recording".into()),
                state: Some("denied".into()),
            }))
        );
        app.finish();
    }

    #[test]
    fn call_rejects_replies_that_break_the_protocol() {
        let line = |s: &str| Reply::Raw(format!("{s}\n").into_bytes());
        let cases = vec![
            line(r#"{"version":1,"id":"other","ok":true,"result":{}}"#),
            line(r#"{"version":2,"id":"x","ok":true,"result":{}}"#),
            line("not json"),
            line(r#"{"version":1,"id":"x","ok":true,"ok":true,"result":{}}"#),
            line(r#"{"version":1,"id":"x","ok":true}"#),
            line(r#"{"version":1,"id":"x","ok":false,"error":{"message":"m"}}"#),
            line(r#"{"version":1,"id":"x","result":{}}"#),
            Reply::Raw(vec![b'a'; MAX_LINE_BYTES + 100]),
            Reply::Raw(format!("{}\n", "a".repeat(MAX_LINE_BYTES + 1)).into_bytes()),
            Reply::Raw(br#"{"version":1,"id":"x","ok":true,"result":{}}"#.to_vec()),
            Reply::Close,
        ];
        for (i, reply) in cases.into_iter().enumerate() {
            let app = FakeApp::spawn(vec![reply]);
            let result = call(&app.socket, "x", "describe", &json!({}), CALL_TIMEOUT);
            assert!(
                matches!(result, Err(CallError::Protocol(_))),
                "case {i}: {result:?}"
            );
            app.finish();
        }
        let app = FakeApp::spawn(vec![Reply::Hang]);
        let started = Instant::now();
        let result = call(
            &app.socket,
            "x",
            "describe",
            &json!({}),
            Duration::from_millis(100),
        );
        assert!(matches!(result, Err(CallError::Protocol(_))), "{result:?}");
        assert!(started.elapsed() < Duration::from_millis(550));
        app.finish();
    }

    #[test]
    fn is_routed_only_for_the_four_methods_without_helpers() {
        let with = |method: &str, extra: Value| {
            let mut v = json!({"id": "a", "method": method, "pid": 1});
            if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            v
        };
        for method in [
            "capture",
            "capture_all_windows",
            "accessibility",
            "accessibility_subtree",
        ] {
            assert!(is_routed(&with(method, json!({}))), "{method}");
            assert!(
                !is_routed(&with(method, json!({"ax_helper": "/x"}))),
                "{method}"
            );
            assert!(
                !is_routed(&with(method, json!({"capture_helper": "/x"}))),
                "{method}"
            );
        }
        for method in [
            "input_source",
            "secure_input",
            "pointer_location",
            "bundle_windows",
        ] {
            assert!(is_routed(&with(method, json!({}))), "{method}");
        }
        for method in [
            "windows",
            "describe",
            "observe_window",
            "action",
            "all_windows",
            "permissions",
            "request_permissions",
        ] {
            assert!(!is_routed(&with(method, json!({}))), "{method}");
        }
        assert!(!is_routed(&json!([])));
        assert!(!is_routed(&json!({"id": "a"})));
    }

    #[test]
    fn route_forwards_capture_and_checks_the_report() {
        for (method, scope) in [
            ("capture", "onscreen_candidates"),
            ("capture_all_windows", "all_window_candidates"),
        ] {
            let report = capture_report(42, 7, "/tmp/a.png", scope);
            let app = FakeApp::spawn(vec![ok("c1", report.clone())]);
            let routed = route(&capture_request(method), &app.socket);
            assert_eq!(
                routed,
                Routed::Done {
                    status: "unknown",
                    result: report
                }
            );
            let seen = app.finish();
            let sent: Value = serde_json::from_str(&seen[0]).unwrap();
            assert_eq!(
                sent,
                json!({"version": 1, "id": "c1", "method": method,
                       "params": {"pid": 42, "window_id": 7, "image": "/tmp/a.png"}})
            );
        }
    }

    #[test]
    fn route_rejects_a_report_for_another_window() {
        let report = capture_report(42, 8, "/tmp/a.png", "onscreen_candidates");
        let app = FakeApp::spawn(vec![ok("c1", report)]);
        match route(&capture_request("capture"), &app.socket) {
            Routed::Failed {
                reason,
                agent_error,
            } => {
                assert!(reason.contains("window_id"), "{reason}");
                assert_eq!(agent_error, None);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        app.finish();
    }

    #[test]
    fn route_forwards_accessibility_and_subtree() {
        let report = accessibility_report("windows_only");
        let app = FakeApp::spawn(vec![ok("a1", report.clone())]);
        let request =
            json!({"id": "a1", "method": "accessibility", "pid": 42, "windows_only": true});
        assert_eq!(
            route(&request, &app.socket),
            Routed::Done {
                status: "ok",
                result: report
            }
        );
        let seen = app.finish();
        let sent: Value = serde_json::from_str(&seen[0]).unwrap();
        assert_eq!(sent["params"], json!({"pid": 42, "windows_only": true}));
        assert!(sent["params"].get("id").is_none() && sent["params"].get("method").is_none());

        let report = subtree_report();
        let app = FakeApp::spawn(vec![ok("s1", report.clone())]);
        let request = json!({"id": "s1", "method": "accessibility_subtree", "pid": 42,
                             "source": "main", "path": [0, 1]});
        assert_eq!(
            route(&request, &app.socket),
            Routed::Done {
                status: "unknown",
                result: report
            }
        );
        let seen = app.finish();
        let sent: Value = serde_json::from_str(&seen[0]).unwrap();
        assert_eq!(
            sent["params"],
            json!({"pid": 42, "source": "main", "path": [0, 1]})
        );
    }

    #[test]
    fn route_forwards_the_observations_and_checks_them() {
        let common = json!({"backend": "fake", "scope": "read_only_unverified",
                            "action_allowed": false, "used_by_input_gate": false});
        let merged = |fields: Value| {
            let mut value = common.clone();
            value
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            value
        };
        let cases = [
            (
                json!({"id": "o1", "method": "input_source"}),
                merged(json!({"input_source_id": "x", "input_mode_id": null,
                    "localized_name": null, "source_type": null, "selection_changed": false})),
                json!({}),
            ),
            (
                json!({"id": "o2", "method": "secure_input"}),
                merged(json!({"secure_event_input_enabled": true})),
                json!({}),
            ),
            (
                json!({"id": "o3", "method": "pointer_location"}),
                merged(json!({"x": 1.0, "y": -2.5, "coordinate_space": "global_top_left_points"})),
                json!({}),
            ),
            (
                json!({"id": "o4", "method": "bundle_windows", "bundle_id": "a.b"}),
                merged(json!({"bundle_id": "a.b", "owner_pids": [], "windows": [],
                    "truncated": false, "window_scope": "onscreen_only",
                    "window_titles": "not_read"})),
                json!({"bundle_id": "a.b"}),
            ),
        ];
        for (request, report, params) in cases {
            let id = request["id"].as_str().unwrap().to_string();
            let app = FakeApp::spawn(vec![ok(&id, report.clone())]);
            assert_eq!(
                route(&request, &app.socket),
                Routed::Done {
                    status: "ok",
                    result: report.clone()
                },
                "{request}"
            );
            let seen = app.finish();
            let sent: Value = serde_json::from_str(&seen[0]).unwrap();
            assert_eq!(sent["method"], request["method"]);
            assert_eq!(sent["params"], params);

            let mut lying = report;
            lying["used_by_input_gate"] = json!(true);
            let app = FakeApp::spawn(vec![ok(&id, lying)]);
            match route(&request, &app.socket) {
                Routed::Failed { reason, .. } => {
                    assert!(reason.contains("used_by_input_gate"), "{reason}")
                }
                other => panic!("expected Failed, got {other:?}"),
            }
            app.finish();
        }
    }

    #[test]
    fn route_validates_before_connecting() {
        let dir = FakeApp::idle();
        let invalid = [
            json!({"id": "a", "method": "capture", "pid": 0, "window_id": 7, "image": "/tmp/a.png"}),
            json!({"id": "a", "method": "capture", "pid": 42, "window_id": 7, "image": "rel.png"}),
            json!({"id": "a", "method": "capture", "pid": 42, "window_id": 7, "image": "/tmp/a.png", "x": 1}),
            json!({"id": "a", "method": "accessibility_subtree", "pid": 42, "source": "x", "path": []}),
            json!({"id": "a", "method": "accessibility", "windows_only": true}),
            json!({"id": "a", "method": "accessibility", "pid": 42, "windows_only": "yes"}),
            json!({"id": "a", "method": "input_source", "select": "com.apple.keylayout.ABC"}),
            json!({"id": "a", "method": "pointer_location", "x": 1}),
            json!({"id": "a", "method": "bundle_windows"}),
            json!({"id": "a", "method": "bundle_windows", "bundle_id": "bad id"}),
            json!({"id": "a", "method": "bundle_windows", "bundle_id": "a.b", "pid": 1}),
        ];
        for request in invalid {
            match route(&request, &dir.socket) {
                Routed::Failed { .. } => {}
                other => panic!("{request}: expected Failed, got {other:?}"),
            }
        }
    }

    #[test]
    fn route_returns_unavailable_when_nothing_listens() {
        let dir = FakeApp::idle();
        match route(&capture_request("capture"), &dir.socket) {
            Routed::Unavailable { reason } => assert!(!reason.is_empty()),
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn route_reports_service_errors_with_their_permission() {
        let app = FakeApp::spawn(vec![Reply::Line(json!({
            "version": 1, "id": "c1", "ok": false,
            "error": {"code": "permission_denied", "message": "denied",
                      "permission": "screen_recording", "state": "denied"}
        }))]);
        match route(&capture_request("capture"), &app.socket) {
            Routed::Failed { agent_error, .. } => assert_eq!(
                agent_error,
                Some(json!({"code": "permission_denied", "message": "denied",
                            "permission": "screen_recording", "state": "denied"}))
            ),
            other => panic!("expected Failed, got {other:?}"),
        }
        app.finish();
    }

    #[test]
    fn input_block_is_checked_key_by_key() {
        let armed = json!({"state": "armed", "backend": "fake",
            "allowed_bundle_ids": ["org.local.rust-computer-use.target-event-fixture"],
            "allowed_keycodes": [18, 36], "expires_unix_seconds": 1.5e9,
            "events_remaining": 2, "halted": false, "verified": false});
        assert_eq!(input_block(&armed), Some(armed.clone()));
        let off = json!({"state": "disabled", "backend": "system"});
        assert_eq!(input_block(&off), Some(off));
        for (key, bad) in [
            ("state", json!("on")),
            ("backend", json!("cgevent")),
            ("allowed_bundle_ids", json!(["com.google.Chrome"])),
            ("allowed_keycodes", json!([128])),
            ("allowed_keycodes", json!([55])),
            ("allowed_keycodes", json!([0])),
            ("events_remaining", json!(-1)),
            ("halted", json!("no")),
            ("verified", json!(true)),
            ("extra", json!(1)),
        ] {
            let mut v = armed.clone();
            v[key] = bad;
            assert_eq!(input_block(&v), None, "{key}");
        }
        assert_eq!(
            input_block(&json!({"state": "armed", "backend": "fake"})),
            None
        );
        assert_eq!(input_block(&json!([])), None);
    }

    #[test]
    fn app_status_reports_running_unavailable_and_error() {
        let describe = json!({"app": "ReachAgent", "app_version": "0.1.0", "protocol_version": 1,
                              "methods": [], "test_mode": true});
        let permissions = json!({"accessibility": "granted", "screen_recording": "denied",
                                 "post_event": "unknown"});
        let app = FakeApp::spawn(vec![ok("status", describe), ok("status", permissions)]);
        let status = app_status(Ok(&app.socket));
        assert_eq!(
            status,
            json!({"state": "running", "socket": app.socket.display().to_string(),
                   "app_version": "0.1.0", "protocol_version": 1, "test_mode": true,
                   "permissions": {"accessibility": "granted", "screen_recording": "denied",
                                   "post_event": "unknown"},
                   "input": {"state": "disabled"}})
        );
        let seen = app.finish();
        let methods: Vec<String> = seen
            .iter()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["method"].to_string())
            .collect();
        assert_eq!(methods, ["\"describe\"", "\"permissions\""]);

        let dir = FakeApp::idle();
        let status = app_status(Ok(&dir.socket));
        assert_eq!(status["state"], "unavailable");
        assert!(status["start"]
            .as_str()
            .unwrap()
            .contains("open -a ReachAgent"));

        let app = FakeApp::spawn(vec![Reply::Raw(b"garbage\n".to_vec())]);
        assert_eq!(app_status(Ok(&app.socket))["state"], "error");
        app.finish();

        let app = FakeApp::spawn(vec![
            ok(
                "status",
                json!({"app_version": "0.1.0", "protocol_version": 1, "test_mode": false}),
            ),
            ok(
                "status",
                json!({"accessibility": "maybe", "screen_recording": "denied", "post_event": "unknown"}),
            ),
        ]);
        assert_eq!(app_status(Ok(&app.socket))["state"], "error");
        app.finish();

        let status = app_status(Err("no home"));
        assert_eq!(status, json!({"state": "unavailable", "reason": "no home"}));
    }

    #[test]
    fn docs_describe_the_agent_route() {
        let cli = include_str!("../docs/AGENT-CLI.md");
        for needle in [
            "agent_unavailable",
            "open -a ReachAgent",
            "REACH_AGENT_SOCKET",
            "狀態碼 3",
        ] {
            assert!(
                cli.contains(needle),
                "docs/AGENT-CLI.md must mention {needle}"
            );
        }
        let readme = include_str!("../README.md");
        let build = include_str!("../docs/BUILD.md");
        assert!(!readme.contains("設計目標（尚未實作）"));
        assert!(!build.contains("設計目標（尚未實作）"));
        assert!(build.contains("AGENT-CLI.md"));
    }
}
