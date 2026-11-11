//! One computer-use JSON request per process. It never sends input itself: a foreground
//! `action` goes to ReachAgent only, which refuses unless it was started with `--enable-input`.
use mac_computer_use::{
    agent_client,
    foreground_input::{self, Status},
    interpretation, observe_with_evidence, overlay_session, Backend, MacBackend, Target,
};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions,
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

const MAX_REQUEST: u64 = 65_536;
const PROTOCOL: &str = "mac-computer-use-agent-v1";

fn fields(v: &Value, required: &[&str]) -> Result<(), String> {
    let object = v.as_object().ok_or("request must be an object")?;
    if object.len() != required.len() + 2
        || !object.contains_key("id")
        || !object.contains_key("method")
        || required.iter().any(|k| !object.contains_key(*k))
        || object
            .keys()
            .any(|k| k != "id" && k != "method" && !required.contains(&k.as_str()))
    {
        return Err("missing or unexpected request fields".into());
    }
    Ok(())
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .ok_or_else(|| format!("invalid {key}"))
}
fn path(v: &Value, key: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(text(v, key)?);
    if !p.is_absolute() {
        return Err(format!("{key} must be absolute"));
    }
    Ok(p)
}
/// `interpret_image` adapters: 1 to `MAX_ADAPTERS` distinct absolute paths, same text rules as `text`.
fn adapter_paths(v: &Value) -> Result<Vec<PathBuf>, String> {
    let items = v["adapters"]
        .as_array()
        .filter(|a| (1..=interpretation::MAX_ADAPTERS).contains(&a.len()))
        .ok_or_else(|| "invalid adapters".to_string())?;
    let mut paths: Vec<PathBuf> = Vec::with_capacity(items.len());
    for item in items {
        let p = PathBuf::from(
            item.as_str()
                .filter(|s| {
                    !s.trim().is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)
                })
                .ok_or_else(|| "invalid adapters".to_string())?,
        );
        if !p.is_absolute() || paths.contains(&p) {
            return Err("invalid adapters".into());
        }
        paths.push(p);
    }
    Ok(paths)
}
fn pid(v: &Value) -> Result<i32, String> {
    v["pid"]
        .as_i64()
        .and_then(|p| i32::try_from(p).ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| "invalid pid".into())
}
fn target(v: &Value) -> Result<Target, String> {
    let window = v["window_id"]
        .as_u64()
        .and_then(|w| u32::try_from(w).ok())
        .ok_or("invalid window_id")?;
    Target::new(pid(v)?, window)
}
fn response(id: Option<&str>, method: Option<&str>, status: &str, result: Value) -> Value {
    json!({"protocol":PROTOCOL,"id":id,"method":method,"status":status,"action_allowed":false,"result":result})
}
fn capture_status(result: &Value) -> Result<&'static str, String> {
    if result["action_allowed"] != false {
        return Err("unsafe capture result".into());
    }
    match result["status"].as_str() {
        Some("captured") if result["decision"] == "unknown" => Ok("unknown"),
        Some("error") => Ok("observation_incomplete"),
        _ => Err("unexpected capture result".into()),
    }
}
fn joint_observation_status(result: &Value) -> Result<&'static str, String> {
    if result["status"] != "unknown" || result["action_allowed"] != false {
        return Err("unsafe combined observation result".into());
    }
    match result["consistency"].as_str() {
        Some("incomplete") => Ok("observation_incomplete"),
        Some("capture_identity_and_cg_geometry_matched_unverified")
            if result["error"].is_null()
                && result["capture"]["status"] == "captured"
                && result["capture"]["action_allowed"] == false
                && result["binding"]["status"] == "unknown"
                && result["binding"]["action_allowed"] == false =>
        {
            Ok("unknown")
        }
        _ => Err("unexpected combined observation result".into()),
    }
}
fn valid_focus_identity(value: &Value) -> bool {
    value.as_object().is_some_and(|v| v.len() == 3)
        && value["pid"]
            .as_u64()
            .is_some_and(|n| n > 0 && n <= i32::MAX as u64)
        && value["started_seconds"].as_i64().is_some_and(|n| n > 0)
        && value["started_microseconds"]
            .as_u64()
            .is_some_and(|n| n < 1_000_000)
}
fn valid_bundle_id(bundle: &str) -> bool {
    !bundle.is_empty()
        && bundle.len() <= 255
        && bundle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}
fn valid_bundle_result(value: &Value, bundle: &str) -> bool {
    if value["status"] != "ok"
        || value["action_allowed"] != false
        || value["bundle_id"] != bundle
        || value["incomplete_count"].as_u64().is_none()
    {
        return false;
    }
    let Some(rows) = value["candidates"]
        .as_array()
        .filter(|rows| rows.len() <= 128)
    else {
        return false;
    };
    let mut seen = std::collections::HashSet::new();
    rows.iter().all(|r| {
        let pid = r["pid"].as_i64();
        pid.is_some_and(|p| p > 0 && p <= i64::from(i32::MAX) && seen.insert(p))
            && r["bundle_id"] == bundle
            && r["process_start_source"] == "kernel_sysctl_kern_proc"
            && r["process_start_unix_seconds"]
                .as_f64()
                .is_some_and(|s| s.is_finite() && s > 0.0)
            && r["executable"].as_str().is_some_and(|s| {
                s.starts_with('/') && s.len() <= 4096 && !s.chars().any(char::is_control)
            })
            && (r["app_bundle_path"].is_null()
                || r["app_bundle_path"].as_str().is_some_and(|s| {
                    s.starts_with('/') && s.len() <= 4096 && !s.chars().any(char::is_control)
                }))
    })
}
fn describe() -> Value {
    let mut d = describe_methods();
    d["methods"]["action"]["foreground"] = json!({
        "fields":["delivery","target","observation","event","evidence_dir"],
        "availability":"disabled by default; only through ReachAgent started with --enable-input; no helper fallback; never through MCP",
        "events":["pointer_move","click (left button)","key_tap (no modifiers)"],
        "statuses":["unsupported","refused","posted_unverified","delivery_unknown","agent_unavailable"]});
    d
}
fn describe_methods() -> Value {
    json!({"methods":{
      "describe":{"fields":[]},
      "sample_isolation":{"fields":["isolation_helper"],"availability":"read-only baseline; not a safety proof"},
      "session_status":{"fields":["session_helper"],"availability":"read-only session flags; isolation unverified"},
      "sample_focus_notifications":{"fields":["notification_helper"],"availability":"read-only focus notifications; not a safety proof"},
      "simulate_input":{"fields":["width","height","events"],"availability":"synthetic only; no OS events"},
      "preview_native_input":{"fields":["width","height","events"],"availability":"synthetic in-memory events only; never posts"},
      "preview_tagged_input":{"fields":["width","height","events","synthetic_window_id"],"availability":"synthetic pointer events and window fields in memory only; never posts"},
      "windows":{"fields":["pid"]},
      "all_windows":{"fields":["pid"],"availability":"read-only all-window candidates; visibility and identity unverified"},
      "observe_all_windows":{"fields":["pid","ax_helper"],"availability":"all-window candidates bracketed by identity samples; unverified"},
      "record_all_windows":{"fields":["pid","ax_helper","output"],"availability":"exclusive evidence record; never input authority"},
      "find_windows":{"fields":["title"],"availability":"exact title candidates only; unverified"},
      "find_bundle":{"fields":["bundle_id","ax_helper"],"availability":"read-only application candidates; unverified"},
      "find_bundle_windows":{"fields":["bundle_id","ax_helper"],"availability":"read-only window candidates; unverified"},
      "find_bundle_all_windows":{"fields":["bundle_id","ax_helper"],"availability":"read-only all-window candidates; omissions and identity remain unverified"},
      "record_bundle_all_windows":{"fields":["bundle_id","ax_helper","output"],"availability":"exclusive read-only bundle inventory evidence; never input authority"},
      "process_identity":{"fields":["pid","ax_helper"],"availability":"read-only repeated process identity; unverified"},
      "accessibility":{"fields":["pid","ax_helper"]},
      "accessibility_subtree":{"fields":["pid","ax_helper","source","path"],"availability":"fresh AX branch; path and window identity unverified"},
      "observe_binding":{"fields":["pid","window_id","ax_helper"]},
      "input_source":{"fields":[],"availability":"read-only current keyboard input source through ReachAgent; never selects one; unverified on a real machine; not used by the input gate"},
      "secure_input":{"fields":[],"availability":"read-only secure event input state through ReachAgent; unverified on a real machine; not used by the input gate"},
      "pointer_location":{"fields":[],"availability":"read-only pointer location through ReachAgent; never moves the pointer; unverified on a real machine; not used by the input gate"},
      "bundle_windows":{"fields":["bundle_id"],"availability":"read-only on-screen windows of one bundle id with layer and bounds through ReachAgent; titles not read; unverified on a real machine; not used by the input gate"},
      "capture":{"fields":["pid","window_id","image","capture_helper","ax_helper"]},
      "capture_all_windows":{"fields":["pid","window_id","image","capture_helper","ax_helper"],"availability":"all-window candidate capture; content and freshness unverified"},
      "record_stream_status":{"fields":["pid","window_id","stream_helper","ax_helper","output"],"availability":"read-only stream metadata evidence; never authorizes input"},
      "capture_stream_image":{"fields":["pid","window_id","stream_helper","ax_helper","image","output"],"availability":"same-sample stream image and metadata; no input authority"},
      "observe_window":{"fields":["pid","window_id","image","capture_helper","ax_helper"],"availability":"read-only evidence"},
      "recognize_text":{"fields":["image","ocr_helper"],"availability":"offline review only"},
      "recognize_image":{"fields":["image","adapter"],"availability":"trusted local image adapter; offline review only"},
      "interpret_image":{"fields":["image","adapters","output"],"availability":"1 to 4 trusted local adapters on one saved PNG; exclusive evidence record; never input authority"},
      "overlay_session":{"fields":["overlay_helper","timeout_ms","steps"],"availability":"display-only cursor overlay for the length of one call; never sends input; never input authority"},
      "action":{"fields":["event"],"availability":"unsupported","event_schema":"legacy form; object with a nonempty kind is accepted only to report unsupported"}},
      "request":"one UTF-8 JSON object on stdin; close stdin after request",
      "id":"required nonempty string, at most 128 bytes",
      "exit_codes":{"0":"read-only result","3":"unknown, unsupported, agent_unavailable or posted_unverified","1":"invalid request, incomplete observation, refused or delivery_unknown"},
      "warning":"helper executable paths are trusted caller inputs; inspect binaries and permissions before use",
      "safety":"no response authorizes input; only a foreground action through an armed ReachAgent can post one event, and its result is never verified"})
}
/// `describe` plus the ReachAgent route: which fields skip the helpers, and the App's state.
fn describe_with_agent(agent: Value) -> Value {
    let mut d = describe();
    for (method, fields, optional) in [
        ("capture", json!(["pid", "window_id", "image"]), None),
        (
            "capture_all_windows",
            json!(["pid", "window_id", "image"]),
            None,
        ),
        (
            "accessibility",
            json!(["pid"]),
            Some(json!(["windows_only"])),
        ),
        (
            "accessibility_subtree",
            json!(["pid", "source", "path"]),
            None,
        ),
        ("input_source", json!([]), None),
        ("secure_input", json!([]), None),
        ("pointer_location", json!([]), None),
        ("bundle_windows", json!(["bundle_id"]), None),
    ] {
        if let Some(entry) = d["methods"][method].as_object_mut() {
            entry.insert("agent_fields".into(), fields);
            if let Some(optional) = optional {
                entry.insert("agent_optional_fields".into(), optional);
            }
        }
    }
    d["agent"] = agent;
    d
}
fn agent_status() -> Value {
    match agent_client::default_socket_path() {
        Ok(socket) => agent_client::app_status(Ok(&socket)),
        Err(reason) => agent_client::app_status(Err(&reason)),
    }
}
/// The four helper-backed methods without helper keys, and the four read-only observations,
/// go to the ReachAgent socket.
fn routed(v: &Value, id: &str, method: &str) -> ExitCode {
    let socket = match agent_client::default_socket_path() {
        Ok(socket) => socket,
        Err(reason) => {
            // A configuration error, not an App that is missing: retrying cannot help.
            return emit_agent(id, method, "error", json!({"reason":reason}));
        }
    };
    match agent_client::route(v, &socket) {
        agent_client::Routed::Done { status, mut result } => {
            if matches!(method, "capture" | "capture_all_windows") {
                let sha = result["image_path"]
                    .as_str()
                    .ok_or_else(|| "agent result has no image_path".to_string())
                    .and_then(|p| mac_computer_use::png_sha256(std::path::Path::new(p)));
                match sha {
                    Ok(hash) => result["image_sha256"] = json!(hash),
                    Err(reason) => {
                        return emit_agent(
                            id,
                            method,
                            "error",
                            json!({"reason":format!("image_sha256: {reason}")}),
                        )
                    }
                }
            }
            emit_agent(id, method, status, result)
        }
        agent_client::Routed::Unavailable { reason } => emit_agent(
            id,
            method,
            "agent_unavailable",
            json!({
                "reason":reason,
                "socket":socket.display().to_string(),
                "start":agent_client::start_hint(&socket),
                "fallback":"none"
            }),
        ),
        agent_client::Routed::Failed {
            reason,
            agent_error,
        } => {
            let mut result = json!({"reason":reason});
            if let Some(error) = agent_error {
                result["agent_error"] = error;
            }
            emit_agent(id, method, "error", result)
        }
    }
}
/// An `action` request in the foreground form (it names `delivery`). The legacy
/// `{event}` form stays `unsupported` and never touches the socket.
fn is_foreground_action(v: &Value) -> bool {
    v["method"] == "action" && v.get("delivery").is_some()
}
/// The keys of every `action` outcome from ReachAgent.
const ACTION_RESULT_KEYS: &[&str] = &[
    "status",
    "action_allowed",
    "gate",
    "reason",
    "attempted",
    "release_attempted",
    "release_confirmed",
    "attempt_file",
    "result_file",
];
/// Check one `action` outcome from ReachAgent. It must be exactly the outcome shape,
/// with one of the four statuses and `action_allowed:false`.
fn action_outcome_status(result: &Value) -> Result<Status, String> {
    let o = result.as_object().ok_or("action result is not an object")?;
    if o.len() != ACTION_RESULT_KEYS.len() || ACTION_RESULT_KEYS.iter().any(|k| !o.contains_key(*k))
    {
        return Err("action result has unexpected fields".into());
    }
    if o["action_allowed"] != false {
        return Err("action result must keep action_allowed false".into());
    }
    let status = match o["status"].as_str() {
        Some("unsupported") => Status::Unsupported,
        Some("refused") => Status::Refused,
        Some("posted_unverified") => Status::PostedUnverified,
        Some("delivery_unknown") => Status::DeliveryUnknown,
        _ => return Err("action result has an unknown status".into()),
    };
    let text_or_null = |k: &str| o[k].is_null() || o[k].as_str().is_some_and(|s| s.len() <= 4096);
    let flag = |k: &str| o[k].is_boolean();
    if !["gate", "reason", "attempt_file", "result_file"]
        .iter()
        .all(|k| text_or_null(k))
        || !["attempted", "release_attempted", "release_confirmed"]
            .iter()
            .all(|k| flag(k))
    {
        return Err("action result has a malformed field".into());
    }
    match status {
        Status::PostedUnverified if o["attempted"] != true || !o["gate"].is_null() => {
            Err("posted_unverified needs attempted true and no gate".into())
        }
        Status::Unsupported | Status::Refused | Status::DeliveryUnknown
            if o["gate"].as_str().is_none() =>
        {
            Err("a stopped action needs a gate".into())
        }
        _ => Ok(status),
    }
}
fn emit_action(id: &str, status: &str, code: u8, result: Value) -> ExitCode {
    println!("{}", response(Some(id), Some("action"), status, result));
    ExitCode::from(code)
}
fn stopped(gate: &str, reason: &str, attempted: Value, detail: Value) -> Value {
    json!({"gate":gate,"reason":reason,"action_allowed":false,"attempted":attempted,"detail":detail})
}
/// Foreground `action`: strict parse, then exactly one call to ReachAgent. There is no
/// helper path and no retry; replay protection and the one-event rule live in the App.
fn foreground_action(v: &Value, id: &str) -> ExitCode {
    let request = match foreground_input::parse_request(v) {
        Ok(request) => request,
        Err(rejection) => {
            return emit_action(
                id,
                rejection.status.as_str(),
                rejection.status.exit_code(),
                stopped(rejection.gate, &rejection.reason, json!(false), Value::Null),
            )
        }
    };
    debug_assert_eq!(request.id, id);
    let socket = match agent_client::default_socket_path() {
        Ok(socket) => socket,
        Err(reason) => return emit_action(id, "error", 1, json!({"reason":reason})),
    };
    let mut params = v.clone();
    if let Some(o) = params.as_object_mut() {
        o.remove("id");
        o.remove("method");
    }
    match agent_client::call(&socket, id, "action", &params, agent_client::CALL_TIMEOUT) {
        Ok(result) => match action_outcome_status(&result) {
            Ok(status) => emit_action(id, status.as_str(), status.exit_code(), result),
            Err(reason) => emit_action(
                id,
                Status::DeliveryUnknown.as_str(),
                Status::DeliveryUnknown.exit_code(),
                stopped(
                    "transport",
                    "agent_result_mismatch",
                    Value::Null,
                    json!({"reason":reason,"note":"an event may have been posted; read the receipts; do not retry"}),
                ),
            ),
        },
        Err(agent_client::CallError::Unavailable(reason)) => emit_action(
            id,
            "agent_unavailable",
            3,
            json!({
                "reason":reason,
                "socket":socket.display().to_string(),
                "start":agent_client::start_hint(&socket),
                "fallback":"none",
                "attempted":false
            }),
        ),
        Err(agent_client::CallError::Agent(f)) if f.code == "unknown_method" => emit_action(
            id,
            Status::Unsupported.as_str(),
            Status::Unsupported.exit_code(),
            stopped(
                "G1",
                "input_disabled",
                json!(false),
                json!({"agent_error":f.message,"start":"start ReachAgent with --enable-input; see docs/AGENT-SERVICE.md"}),
            ),
        ),
        Err(agent_client::CallError::Agent(f)) => {
            let mut error = json!({"code": f.code, "message": f.message});
            if let Some(permission) = &f.permission {
                error["permission"] = json!(permission);
            }
            if let Some(state) = &f.state {
                error["state"] = json!(state);
            }
            emit_action(
                id,
                "error",
                1,
                json!({"reason":format!("{}: {}", f.code, f.message),"agent_error":error,"attempted":false}),
            )
        }
        // The request line may have reached the App: delivery is unknown, never retried.
        Err(agent_client::CallError::Protocol(reason)) => emit_action(
            id,
            Status::DeliveryUnknown.as_str(),
            Status::DeliveryUnknown.exit_code(),
            stopped(
                "transport",
                "reply_lost",
                Value::Null,
                json!({"reason":reason,"note":"an event may have been posted; read the receipts; do not retry"}),
            ),
        ),
    }
}
fn emit_agent(id: &str, method: &str, status: &str, result: Value) -> ExitCode {
    println!("{}", response(Some(id), Some(method), status, result));
    match status {
        "ok" => ExitCode::SUCCESS,
        "unknown" | "unsupported" | "agent_unavailable" => ExitCode::from(3),
        _ => ExitCode::FAILURE,
    }
}
fn execute<B: Backend>(v: &Value, backend: &B) -> Result<(&'static str, Value), String> {
    match text(v, "method")? {
        "describe" => {
            fields(v, &[])?;
            Ok(("ok", describe_with_agent(agent_status())))
        }
        "simulate_input" | "preview_native_input" | "preview_tagged_input" => {
            if v["method"] == "preview_tagged_input" {
                fields(v, &["width", "height", "events", "synthetic_window_id"])?;
            } else {
                fields(v, &["width", "height", "events"])?;
            }
            let width = v["width"].as_f64().ok_or("invalid width")?;
            let height = v["height"].as_f64().ok_or("invalid height")?;
            let events = v["events"].as_array().ok_or("invalid events")?;
            let result = if v["method"] == "preview_tagged_input" {
                let id = v["synthetic_window_id"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or("invalid synthetic window ID")?;
                mac_computer_use::simulate::run_native_tagged(width, height, events, id)?
            } else if v["method"] == "preview_native_input" {
                mac_computer_use::simulate::run_native(width, height, events)?
            } else {
                mac_computer_use::simulate::run(width, height, events)?
            };
            Ok(("unknown", result))
        }
        "session_status" => {
            fields(v, &["session_helper"])?;
            let output = mac_computer_use::bounded_process::run(
                std::process::Command::new(path(v, "session_helper")?).arg("--sample"),
                std::time::Duration::from_secs(5),
            )?;
            let value = mac_computer_use::strict_json::from_slice(&output.stdout)
                .map_err(|e| format!("invalid session JSON: {e}"))?;
            let obj = value.as_object().ok_or("invalid session result")?;
            let valid_flag = |name: &str| value[name].is_null() || value[name].is_boolean();
            if !output.status.success()
                || obj.len() != 7
                || [
                    "status",
                    "action_allowed",
                    "input_posted",
                    "dictionary_available",
                    "on_console",
                    "login_done",
                    "note",
                ]
                .iter()
                .any(|key| !obj.contains_key(*key))
                || value["status"] != "unknown"
                || value["action_allowed"] != false
                || value["input_posted"] != false
                || !value["dictionary_available"].is_boolean()
                || !valid_flag("on_console")
                || !valid_flag("login_done")
                || value["note"]
                    .as_str()
                    .is_none_or(|note| note.is_empty() || note.len() > 256)
                || (value["dictionary_available"] == false
                    && (!value["on_console"].is_null() || !value["login_done"].is_null()))
            {
                return Err("invalid or unsafe session result".into());
            }
            Ok(("unknown", value))
        }
        "sample_isolation" => {
            fields(v, &["isolation_helper"])?;
            let helper = path(v, "isolation_helper")?;
            let output = mac_computer_use::bounded_process::run(
                std::process::Command::new(helper).arg("--sample"),
                std::time::Duration::from_secs(5),
            )?;
            let value = mac_computer_use::strict_json::from_slice(&output.stdout)
                .map_err(|e| format!("invalid isolation JSON: {e}"))?;
            if !output.status.success() {
                if value["status"] == "unknown"
                    && value["action_allowed"] == false
                    && value["input_posted"] == false
                    && value["reason"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 128)
                {
                    return Ok(("unknown", value));
                }
                return Err("isolation sampling failed with invalid result".into());
            }
            if !matches!(
                value["status"].as_str(),
                Some("sampled" | "sampled_partial")
            ) || value["action_allowed"] != false
                || value["input_posted"] != false
                || !matches!(
                    value["focus_scope"].as_str(),
                    Some("system" | "front_app_only" | "mixed")
                )
                || value["system_focus_sampled"].as_bool() != Some(value["focus_scope"] == "system")
                || !value["timing_within_limit"].is_boolean()
                || (value["status"] == "sampled")
                    != (value["focus_scope"] == "system" && value["timing_within_limit"] == true)
                || (value["focus_scope"] == "mixed" && !value["focus_unchanged"].is_null())
                || (value["focus_scope"] != "mixed" && !value["focus_unchanged"].is_boolean())
                || value["samples"].as_u64() != Some(10)
                || [
                    "foreground_unchanged",
                    "cursor_unchanged",
                    "window_order_unchanged",
                ]
                .iter()
                .any(|key| !value[*key].is_boolean())
            {
                return Err("invalid isolation result".into());
            }
            Ok(("unknown", value))
        }
        "sample_focus_notifications" => {
            fields(v, &["notification_helper"])?;
            let helper = path(v, "notification_helper")?;
            let output = mac_computer_use::bounded_process::run(
                std::process::Command::new(helper).arg("--sample"),
                std::time::Duration::from_secs(5),
            )?;
            let value = mac_computer_use::strict_json::from_slice(&output.stdout)
                .map_err(|e| format!("invalid focus notification JSON: {e}"))?;
            if value["action_allowed"] != false || value["input_posted"] != false {
                return Err("unsafe focus notification result".into());
            }
            if !output.status.success() {
                if value["status"] == "unknown"
                    && value["reason"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 128)
                {
                    return Ok(("unknown", value));
                }
                return Err("focus notification sampling failed with invalid result".into());
            }
            let names = ["focused_element", "focused_window", "deactivated"];
            let reg = value["registration"]
                .as_object()
                .ok_or("missing focus notification registrations")?;
            let counts = value["counts"]
                .as_object()
                .ok_or("missing focus notification counts")?;
            if reg.len() != names.len()
                || counts.len() != names.len()
                || names.iter().any(|name| {
                    !reg.get(*name).is_some_and(Value::is_boolean)
                        || counts.get(*name).and_then(Value::as_u64).is_none()
                })
                || !value["frontmost_same_at_end"].is_boolean()
                || !valid_focus_identity(&value["foreground_identity"])
                || (!value["foreground_identity_at_end"].is_null()
                    && !valid_focus_identity(&value["foreground_identity_at_end"]))
                || (value["frontmost_same_at_end"] == true)
                    != (!value["foreground_identity_at_end"].is_null()
                        && value["foreground_identity"] == value["foreground_identity_at_end"])
                || !value["duration_seconds"]
                    .as_f64()
                    .is_some_and(|t| t.is_finite() && (1.0..=3.0).contains(&t))
            {
                return Err("invalid focus notification result".into());
            }
            let all_registered = names.iter().all(|name| reg[*name] == true);
            if (all_registered && value["status"] != "sampled")
                || (!all_registered && value["status"] != "sampled_partial")
            {
                return Err("inconsistent focus notification registrations".into());
            }
            Ok(("unknown", value))
        }
        "action" => {
            fields(v, &["event"])?;
            let event = v["event"].as_object().ok_or("event must be an object")?;
            let kind = event
                .get("kind")
                .and_then(Value::as_str)
                .filter(|s| {
                    !s.trim().is_empty() && s.len() <= 64 && !s.chars().any(char::is_control)
                })
                .ok_or("invalid event kind")?;
            Ok((
                "unsupported",
                json!({"event_kind":kind,"event_schema":"unvalidated","reason":"input is not implemented or verified; no event was sent"}),
            ))
        }
        "find_windows" => {
            fields(v, &["title"])?;
            let title = text(v, "title")?;
            let windows = backend.windows(None)?;
            let matches = windows.iter().filter(|w| w.title.as_deref() == Some(title)).map(|w| json!({
                "pid":w.target.pid,"window_id":w.target.window_id,"layer":w.layer,
                "surface_class":w.surface_class(),"bounds_points":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height}
            })).collect::<Vec<_>>();
            Ok((
                "unknown",
                json!({"candidate_count":matches.len(),"candidates":matches,
                "action_allowed":false,"note":"exact title is not identity; list can be incomplete or stale and untitled windows cannot match"}),
            ))
        }
        "windows" | "all_windows" => {
            fields(v, &["pid"])?;
            let pid = pid(v)?;
            let all = text(v, "method")? == "all_windows";
            let (windows, omitted) = if all {
                backend.all_windows(pid)?
            } else {
                (backend.windows(Some(pid))?, 0)
            };
            if windows.iter().any(|w| w.target.pid != pid) {
                return Err("backend returned another process".into());
            }
            Ok((
                if all { "unknown" } else { "ok" },
                json!({"scope":if all {"all_window_candidates"} else {"onscreen_candidates"},"omitted_invalid_count":omitted,"action_allowed":false,"coordinate_system":"quartz_screen_points_top_left","windows":windows.iter().map(|w| json!({
                "pid":w.target.pid,"window_id":w.target.window_id,"layer":w.layer,"title":w.title,
                "surface_class":w.surface_class(),"bounds_points":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height}
            })).collect::<Vec<_>>(),"note":"window identity, visibility, capture eligibility, completeness and input safety are unverified"}),
            ))
        }
        "record_all_windows" => {
            fields(v, &["pid", "ax_helper", "output"])?;
            let target_pid = pid(v)?;
            let helper = path(v, "ax_helper")?;
            let output = path(v, "output")?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&output)
                .map_err(|e| format!("cannot reserve evidence: {e}"))?;
            let query = json!({"id":v["id"],"method":"observe_all_windows","pid":target_pid,"ax_helper":helper});
            let mut record = json!({"schema":"cu-all-windows-evidence-v1","action_allowed":false,"observation":null,"error":null});
            match execute(&query, backend) {
                Ok((_, result)) => record["observation"] = result,
                Err(error) => record["error"] = error.into(),
            }
            let complete = record["observation"]["same_identity_samples"] == true
                && record["observation"]["inventory"]["omitted_invalid_count"] == 0;
            record["observation_complete"] = complete.into();
            let bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("evidence may be partial at {}: {e}", output.display()))?;
            Ok((
                if complete {
                    "unknown"
                } else {
                    "observation_incomplete"
                },
                json!({"action_allowed":false,"evidence_file":output,"observation_complete":complete,"record":record}),
            ))
        }
        "capture_stream_image" => {
            fields(
                v,
                &[
                    "pid",
                    "window_id",
                    "stream_helper",
                    "ax_helper",
                    "image",
                    "output",
                ],
            )?;
            let target = target(v)?;
            let helper = path(v, "stream_helper")?;
            let _ = path(v, "ax_helper")?;
            let image = path(v, "image")?;
            let output = path(v, "output")?;
            let record =
                mac_computer_use::stream_image::record(backend, target, &helper, &image, &output)?;
            let captured = record["status"] == "captured" && record["action_allowed"] == false;
            Ok((
                if captured {
                    "unknown"
                } else {
                    "observation_incomplete"
                },
                json!({"action_allowed":false,"evidence_file":output,"captured":captured,"record":record}),
            ))
        }
        "record_stream_status" => {
            fields(
                v,
                &["pid", "window_id", "stream_helper", "ax_helper", "output"],
            )?;
            let target = target(v)?;
            let helper = path(v, "stream_helper")?;
            let _ = path(v, "ax_helper")?;
            let output = path(v, "output")?;
            let record =
                mac_computer_use::stream_status::record(backend, target, &helper, &output)?;
            let complete = record["sample_complete"] == true && record["error"].is_null();
            Ok((
                if complete {
                    "unknown"
                } else {
                    "observation_incomplete"
                },
                json!({"action_allowed":false,"evidence_file":output,"sample_complete":complete,"record":record}),
            ))
        }
        "record_bundle_all_windows" => {
            fields(v, &["bundle_id", "ax_helper", "output"])?;
            let bundle = text(v, "bundle_id")?;
            if !valid_bundle_id(bundle) {
                return Err("invalid bundle_id".into());
            }
            let helper = path(v, "ax_helper")?;
            let output = path(v, "output")?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&output)
                .map_err(|e| format!("cannot reserve evidence: {e}"))?;
            let query = json!({"id":v["id"],"method":"find_bundle_all_windows","bundle_id":bundle,"ax_helper":helper});
            let mut record = json!({"schema":"cu-bundle-all-windows-evidence-v1","action_allowed":false,"observation":null,"error":null});
            match execute(&query, backend) {
                Ok((_, result)) => record["observation"] = result,
                Err(error) => record["error"] = error.into(),
            }
            let observation = &record["observation"];
            let complete = observation["same_complete_samples"] == true
                && observation["all_inventory_calls_completed"] == true
                && observation["inventories"]
                    .as_array()
                    .is_some_and(|items| items.iter().all(|i| i["omitted_invalid_count"] == 0));
            record["query_complete"] = complete.into();
            let bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("evidence may be partial at {}: {e}", output.display()))?;
            Ok((
                if complete {
                    "unknown"
                } else {
                    "observation_incomplete"
                },
                json!({"action_allowed":false,"evidence_file":output,"query_complete":complete,"record":record}),
            ))
        }
        "observe_all_windows" => {
            fields(v, &["pid", "ax_helper"])?;
            let target_pid = pid(v)?;
            let helper = path(v, "ax_helper")?;
            let identity = json!({"id":v["id"],"method":"process_identity","pid":target_pid,"ax_helper":helper});
            let (_, before) = execute(&identity, backend)?;
            let mut result = json!({"action_allowed":false,"identity_before":before,"inventory":null,"identity_after":null,
                "same_identity_samples":false,"note":"sequential samples do not prove continuous identity, window binding, visibility, capture eligibility or input safety"});
            if result["identity_before"]["same_identity_samples"] != true {
                result["reason"] = result["identity_before"]["reason"]
                    .as_str()
                    .map(|r| format!("initial identity incomplete: {r}"))
                    .unwrap_or_else(|| "initial identity samples differ".into())
                    .into();
                return Ok(("unknown", result));
            }
            let query = json!({"id":v["id"],"method":"all_windows","pid":target_pid});
            match execute(&query, backend) {
                Ok((_, inventory)) => result["inventory"] = inventory,
                Err(error) => {
                    result["reason"] = format!("window inventory failed: {error}").into();
                    return Ok(("unknown", result));
                }
            }
            match execute(&identity, backend) {
                Ok((_, after)) => {
                    let equal = after["same_identity_samples"] == true
                        && [
                            "pid",
                            "process_start_unix_seconds",
                            "executable",
                            "bundle_id",
                        ]
                        .iter()
                        .all(|k| {
                            result["identity_before"]["identity_after"][*k]
                                == after["identity_before"][*k]
                        });
                    result["same_identity_samples"] = equal.into();
                    if !equal {
                        result["reason"] = after["reason"]
                            .as_str()
                            .map(|r| format!("final identity incomplete: {r}"))
                            .unwrap_or_else(|| "identity samples differ".into())
                            .into();
                    }
                    result["identity_after"] = after;
                }
                Err(error) => {
                    result["reason"] = format!("final identity lookup failed: {error}").into()
                }
            }
            Ok(("unknown", result))
        }
        "find_bundle_windows" | "find_bundle_all_windows" => {
            let all = v["method"] == "find_bundle_all_windows";
            fields(v, &["bundle_id", "ax_helper"])?;
            let bundle = text(v, "bundle_id")?;
            let helper = path(v, "ax_helper")?;
            let lookup = json!({"id":v["id"], "method":"find_bundle", "bundle_id":bundle, "ax_helper":helper});
            let (_, first) = execute(&lookup, backend)?;
            let mut result = json!({"action_allowed":false,"before":first,"windows":[],"after":null,"same_complete_samples":false,
                "note":"candidates are sequential observations, not window binding or input authority"});
            if all {
                result["inventory_scope"] = "all_window_candidates".into();
                result["inventories"] = json!([]);
                result["all_inventory_calls_completed"] = false.into();
            }
            if first["same_complete_samples"] != true {
                result["reason"] = "initial process samples incomplete or changed".into();
                return Ok(("unknown", result));
            }
            let candidates = first["after"]["candidates"]
                .as_array()
                .ok_or("missing candidates")?;
            if candidates.len() > 16 {
                result["reason"] = "too many process candidates".into();
                return Ok(("unknown", result));
            }
            for candidate in candidates {
                let pid = candidate["pid"]
                    .as_i64()
                    .and_then(|p| i32::try_from(p).ok())
                    .ok_or("invalid candidate pid")?;
                let inventory = if all {
                    backend.all_windows(pid)
                } else {
                    backend.windows(Some(pid)).map(|w| (w, 0))
                };
                let (windows, omitted) = match inventory {
                    Ok((w, omitted)) if w.iter().all(|w| w.target.pid == pid) => (w, omitted),
                    _ => {
                        result["reason"] =
                            "window inventory failed or returned another process".into();
                        return Ok(("unknown", result));
                    }
                };
                if all {
                    result["inventories"].as_array_mut().unwrap().push(json!({"pid":pid,"omitted_invalid_count":omitted,"candidate_count":windows.len()}));
                }
                let rows = windows.iter().map(|w| json!({"pid":w.target.pid,"window_id":w.target.window_id,"title":w.title,
                    "layer":w.layer,"surface_class":w.surface_class(),"bounds_points":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height}})).collect::<Vec<_>>();
                result["windows"].as_array_mut().unwrap().extend(rows);
            }
            if all {
                result["all_inventory_calls_completed"] = true.into();
            }
            match execute(&lookup, backend) {
                Ok((_, after)) => {
                    result["same_complete_samples"] = (after["same_complete_samples"] == true
                        && first["after"] == after["before"])
                        .into();
                    result["after"] = after;
                }
                Err(error) => {
                    result["reason"] = format!("final process lookup failed: {error}").into()
                }
            }
            Ok(("unknown", result))
        }
        "find_bundle" => {
            fields(v, &["bundle_id", "ax_helper"])?;
            let bundle = text(v, "bundle_id")?;
            if !valid_bundle_id(bundle) {
                return Err("invalid bundle_id".into());
            }
            let helper = path(v, "ax_helper")?;
            let query = || -> Result<Value, String> {
                let output = mac_computer_use::bounded_process::run(
                    std::process::Command::new(&helper)
                        .arg("--find-bundle")
                        .arg(bundle),
                    std::time::Duration::from_secs(15),
                )?;
                if !output.status.success() {
                    return Err("bundle lookup failed".into());
                }
                let value = mac_computer_use::strict_json::from_slice(&output.stdout)
                    .map_err(|e| format!("invalid bundle lookup JSON: {e}"))?;
                if !valid_bundle_result(&value, bundle) {
                    return Err("invalid bundle lookup evidence".into());
                }
                Ok(value)
            };
            let before = query()?;
            match query() {
                Ok(after) => {
                    let consistent = before == after && before["incomplete_count"] == 0;
                    Ok((
                        "unknown",
                        json!({"action_allowed":false,"same_complete_samples":consistent,
                        "before":before,"after":after,"note":"samples may be incomplete or stale; recheck identity and windows before use"}),
                    ))
                }
                Err(error) => Ok((
                    "unknown",
                    json!({"action_allowed":false,"same_complete_samples":false,
                    "before":before,"after":null,"reason":format!("second process sample failed: {error}")}),
                )),
            }
        }
        "process_identity" => {
            fields(v, &["pid", "ax_helper"])?;
            let _ = path(v, "ax_helper")?;
            let pid = pid(v)?;
            let before = backend.identity(pid)?;
            let valid = |identity: &Value| {
                identity["status"] == "ok"
                    && identity["action_allowed"] == false
                    && identity["pid"].as_i64() == Some(i64::from(pid))
                    && identity["process_start_unix_seconds"]
                        .as_f64()
                        .is_some_and(|v| v.is_finite() && v > 0.0)
                    && identity["executable"].as_str().is_some_and(|v| {
                        v.starts_with('/') && v.len() <= 4096 && !v.chars().any(char::is_control)
                    })
                    && (identity["bundle_id"].is_null()
                        || identity["bundle_id"].as_str().is_some_and(|v| {
                            !v.is_empty() && v.len() <= 512 && !v.chars().any(char::is_control)
                        }))
            };
            if !valid(&before) {
                return Err("invalid process identity evidence".into());
            }
            let after = match backend.identity(pid) {
                Ok(after) if valid(&after) => after,
                Ok(_) => {
                    return Ok((
                        "unknown",
                        json!({"action_allowed":false,"identity_before":before,
                    "identity_after":null,"same_identity_samples":false,"reason":"invalid second process identity evidence"}),
                    ))
                }
                Err(error) => {
                    return Ok((
                        "unknown",
                        json!({"action_allowed":false,"identity_before":before,
                    "identity_after":null,"same_identity_samples":false,"reason":format!("second process identity lookup failed: {error}")}),
                    ))
                }
            };
            let fields = [
                "pid",
                "process_start_unix_seconds",
                "executable",
                "bundle_id",
            ];
            let same = fields.iter().all(|key| before[*key] == after[*key]);
            Ok((
                "unknown",
                json!({"action_allowed":false,"identity_before":before,"identity_after":after,
                "same_identity_samples":same,"note":"matching samples do not prove continuous identity or authorize input"}),
            ))
        }
        "accessibility_subtree" => {
            fields(v, &["pid", "ax_helper", "source", "path"])?;
            let _ = path(v, "ax_helper")?;
            let source = text(v, "source")?;
            if !matches!(source, "main" | "focused" | "windows") {
                return Err("invalid subtree source".into());
            }
            let values = v["path"]
                .as_array()
                .filter(|a| a.len() <= 8)
                .ok_or("invalid subtree path")?;
            let indexes = values
                .iter()
                .map(|v| {
                    v.as_u64()
                        .filter(|i| *i < 160)
                        .map(|i| i as usize)
                        .ok_or("invalid subtree index")
                })
                .collect::<Result<Vec<_>, _>>()?;
            if source == "windows" && indexes.first().is_none_or(|i| *i >= 32) {
                return Err("invalid window index".into());
            }
            let result = backend.accessibility_subtree(pid(v)?, source, &indexes)?;
            if result["status"] != "ok"
                || result["action_allowed"] != false
                || result["scope"] != "read_only_subtree_unverified"
                || result["cg_window_binding"] != "unverified"
                || result["node"].as_object().is_none()
                || result["truncated"].as_bool().is_none()
                || result["pid"].as_i64() != Some(i64::from(pid(v)?))
                || result["source"] != source
                || result["path"] != json!(indexes)
            {
                return Err("unsafe or mismatched subtree result".into());
            }
            Ok(("unknown", result))
        }
        "accessibility" => {
            fields(v, &["pid", "ax_helper"])?;
            let _ = path(v, "ax_helper")?;
            let pid = pid(v)?;
            let result = backend.accessibility(pid)?;
            if result["status"] != "ok"
                || result["action_allowed"] != false
                || result["pid"].as_i64() != Some(pid.into())
            {
                return Err("unsafe or mismatched accessibility result".into());
            }
            Ok(("ok", result))
        }
        "observe_binding" => {
            fields(v, &["pid", "window_id", "ax_helper"])?;
            let _ = path(v, "ax_helper")?;
            let result = mac_computer_use::live_ax_cg::observe(backend, target(v)?);
            if result["status"] != "unknown" || result["action_allowed"] != false {
                return Err("unsafe or unexpected binding result".into());
            }
            Ok(("unknown", result))
        }
        "recognize_image" => {
            fields(v, &["image", "adapter"])?;
            let image = path(v, "image")?;
            let helper = path(v, "adapter")?;
            let value = mac_computer_use::recognize_image(&image, &helper)?;
            Ok(("unknown", value))
        }
        "recognize_text" => {
            fields(v, &["image", "ocr_helper"])?;
            let image = path(v, "image")?;
            let helper = path(v, "ocr_helper")?;
            let value = mac_computer_use::recognize_text(&image, &helper)?;
            Ok(("unknown", value))
        }
        "interpret_image" => {
            fields(v, &["image", "adapters", "output"])?;
            let image = path(v, "image")?;
            let output = path(v, "output")?;
            let adapters = adapter_paths(v)?;
            // Fails before the evidence file exists, so a bad image leaves no record.
            let before = mac_computer_use::png_sha256(&image)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&output)
                .map_err(|e| format!("cannot reserve evidence: {e}"))?;
            let mut interpretations = Vec::with_capacity(adapters.len());
            for adapter in &adapters {
                // The adapter digest describes the file as read now; it does not prove trust.
                let (adapter_sha256, adapter_sha256_error) =
                    match mac_computer_use::regular_file_sha256(adapter) {
                        Ok(h) => (json!(h), Value::Null),
                        Err(e) => (Value::Null, json!(e)),
                    };
                let (entry, raw) = match mac_computer_use::recognize_image(&image, adapter) {
                    Ok(raw) => (interpretation::classify(&raw), raw),
                    Err(reason) => (interpretation::failed(&reason), Value::Null),
                };
                interpretations.push(json!({
                    "adapter": adapter,
                    "adapter_sha256": adapter_sha256,
                    "adapter_sha256_error": adapter_sha256_error,
                    "outcome": entry["outcome"],
                    "reason": entry["reason"],
                    "interpreter": entry["interpreter"],
                    "answer": entry["answer"],
                    "raw": raw,
                }));
            }
            let after = mac_computer_use::png_sha256(&image).ok();
            let image_stable = after.as_deref() == Some(before.as_str());
            let agreement = interpretation::agreement(&interpretations);
            let complete = interpretation::complete(&interpretations, image_stable);
            let record = json!({"schema":interpretation::SCHEMA,"action_allowed":false,
                "image":image,"image_sha256":before,"image_sha256_after":after,
                "image_stable":image_stable,"interpretations":interpretations,
                "agreement":agreement,"interpretation_complete":complete,
                "note":"answers, scores, boxes and labels are unverified review data; nothing here authorizes input"});
            let bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("evidence may be partial at {}: {e}", output.display()))?;
            Ok((
                if complete {
                    "unknown"
                } else {
                    "interpretation_incomplete"
                },
                json!({"action_allowed":false,"evidence_file":output,
                    "interpretation_complete":complete,"agreement":agreement,"record":record}),
            ))
        }
        "overlay_session" => {
            fields(v, &["overlay_helper", "timeout_ms", "steps"])?;
            let helper = path(v, "overlay_helper")?;
            let timeout = v["timeout_ms"]
                .as_u64()
                .filter(|ms| {
                    (overlay_session::MIN_TIMEOUT_MS..=overlay_session::MAX_TIMEOUT_MS).contains(ms)
                })
                .ok_or("invalid timeout_ms")?;
            let steps = overlay_session::parse_steps(&v["steps"])?;
            let result = overlay_session::run(&helper, &steps, Duration::from_millis(timeout));
            let status = if result["completed"] == true {
                "unknown"
            } else {
                "overlay_incomplete"
            };
            Ok((status, result))
        }
        "observe_window" => {
            fields(
                v,
                &["pid", "window_id", "image", "capture_helper", "ax_helper"],
            )?;
            let _ = (path(v, "capture_helper")?, path(v, "ax_helper")?);
            let result = mac_computer_use::joint_observation::observe(
                backend,
                target(v)?,
                &path(v, "image")?,
            )?;
            let status = joint_observation_status(&result)?;
            Ok((status, result))
        }
        "capture" | "capture_all_windows" => {
            fields(
                v,
                &["pid", "window_id", "image", "capture_helper", "ax_helper"],
            )?;
            let _ = (path(v, "capture_helper")?, path(v, "ax_helper")?);
            let image = path(v, "image")?;
            let result = if text(v, "method")? == "capture_all_windows" {
                mac_computer_use::observe_all_with_evidence(backend, target(v)?, &image)?
            } else {
                observe_with_evidence(backend, target(v)?, &image)?
            };
            let status = capture_status(&result)?;
            Ok((status, result))
        }
        _ => Err("unknown method".into()),
    }
}
pub fn main() -> ExitCode {
    let mut bytes = Vec::new();
    let read = io::stdin()
        .lock()
        .take(MAX_REQUEST + 1)
        .read_to_end(&mut bytes);
    let request: Result<Value, String> =
        read.map_err(|e| format!("stdin error: {e}")).and_then(|_| {
            if bytes.len() as u64 > MAX_REQUEST {
                Err("request too large".into())
            } else {
                mac_computer_use::strict_json::from_slice(&bytes)
                    .map_err(|_| "invalid JSON or duplicate object key".into())
            }
        });
    let v = match request {
        Ok(v) => v,
        Err(e) => {
            println!("{}", response(None, None, "error", json!({"reason":e})));
            return ExitCode::FAILURE;
        }
    };
    let id = v["id"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control));
    let method = v["method"].as_str();
    if id.is_none() || method.is_none() || std::env::args().len() != 1 {
        println!(
            "{}",
            response(
                id,
                method,
                "error",
                json!({"reason":"invalid id, method, or command arguments"})
            )
        );
        return ExitCode::FAILURE;
    }
    if is_foreground_action(&v) {
        if let Some(id) = id {
            return foreground_action(&v, id);
        }
    }
    if agent_client::is_routed(&v) {
        if let (Some(id), Some(method)) = (id, method) {
            return routed(&v, id, method);
        }
    }
    let result = (|| -> Result<(&'static str, Value), String> {
        // Unused placeholders are never executed by describe, action, or windows.
        let capture = if matches!(
            method,
            Some("capture" | "capture_all_windows" | "observe_window")
        ) {
            path(&v, "capture_helper")?
        } else {
            PathBuf::from("/unused-capture-helper")
        };
        let ax = if matches!(
            method,
            Some(
                "accessibility"
                    | "accessibility_subtree"
                    | "find_bundle"
                    | "find_bundle_windows"
                    | "find_bundle_all_windows"
                    | "record_bundle_all_windows"
                    | "record_stream_status"
                    | "capture_stream_image"
                    | "process_identity"
                    | "observe_all_windows"
                    | "record_all_windows"
                    | "observe_binding"
                    | "capture"
                    | "capture_all_windows"
                    | "observe_window"
            )
        ) {
            path(&v, "ax_helper")?
        } else {
            PathBuf::from("/unused-ax-helper")
        };
        let backend = MacBackend::new(capture, ax)?;
        execute(&v, &backend)
    })();
    match result {
        Ok((status, payload)) => {
            println!("{}", response(id, method, status, payload));
            match status {
                "ok" => ExitCode::SUCCESS,
                "unknown" | "unsupported" => ExitCode::from(3),
                _ => ExitCode::FAILURE,
            }
        }
        Err(reason) => {
            println!(
                "{}",
                response(id, method, "error", json!({"reason":reason}))
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mac_computer_use::{Rect, Window};
    use std::{cell::Cell, path::Path};
    struct Fake {
        calls: Cell<u32>,
    }
    impl Backend for Fake {
        fn identity(&self, pid: i32) -> Result<Value, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(
                json!({"status":"ok","action_allowed":pid == 44,"pid":pid,"process_start_unix_seconds":if pid == 43 { f64::from(self.calls.get()) } else { 123.0 },"executable":"/test","bundle_id":"test.app"}),
            )
        }
        fn windows(&self, pid: Option<i32>) -> Result<Vec<Window>, String> {
            self.calls.set(self.calls.get() + 1);
            let mut windows = vec![Window {
                target: Target::new(pid.unwrap_or(42), 9).unwrap(),
                bounds: Rect {
                    x: 0.,
                    y: 0.,
                    width: 10.,
                    height: 10.,
                },
                layer: Some(0),
                title: Some("test".into()),
            }];
            if pid.is_none() {
                let mut duplicate = windows[0].clone();
                duplicate.target = Target::new(43, 10).unwrap();
                windows.push(duplicate);
                let mut untitled = windows[0].clone();
                untitled.title = None;
                windows.push(untitled);
            }
            Ok(windows)
        }
        fn all_windows(&self, pid: i32) -> Result<(Vec<Window>, usize), String> {
            let mut windows = self.windows(Some(pid))?;
            windows[0].target = Target::new(pid, 99).unwrap();
            Ok((windows, 1))
        }
        fn accessibility_subtree(
            &self,
            pid: i32,
            source: &str,
            path: &[usize],
        ) -> Result<Value, String> {
            self.calls.set(self.calls.get() + 1);
            Ok(
                json!({"status":"ok","action_allowed":false,"scope":"read_only_subtree_unverified","cg_window_binding":"unverified","pid":pid,"source":source,"path":path,"node":{},"truncated":false}),
            )
        }
        fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
            self.calls.set(self.calls.get() + 1);
            Err("unavailable".into())
        }
    }
    #[test]
    fn combined_observation_distinguishes_incomplete_from_unknown() {
        let incomplete = json!({"status":"unknown", "action_allowed":false,
            "consistency":"incomplete", "error":"capture unavailable"});
        assert_eq!(
            joint_observation_status(&incomplete).unwrap(),
            "observation_incomplete"
        );
        let mut complete = json!({"status":"unknown", "action_allowed":false,
            "consistency":"capture_identity_and_cg_geometry_matched_unverified",
            "capture":{"status":"captured", "action_allowed":false},
            "binding":{"status":"unknown", "action_allowed":false}});
        assert_eq!(joint_observation_status(&complete).unwrap(), "unknown");
        complete["binding"]["action_allowed"] = true.into();
        assert!(joint_observation_status(&complete).is_err());
        complete["binding"]["action_allowed"] = false.into();
        complete["error"] = "partial failure".into();
        assert!(joint_observation_status(&complete).is_err());
    }

    #[test]
    fn typed_events_are_unsupported_without_touching_backend() {
        let f = Fake {
            calls: Cell::new(0),
        };
        for kind in ["pointer_click", "pointer_move", "key_press", "future_event"] {
            let r = json!({"id":"1","method":"action","event":{"kind":kind,"other":"unvalidated"}});
            let (status, result) = execute(&r, &f).unwrap();
            assert_eq!(status, "unsupported");
            assert_eq!(result["event_schema"], "unvalidated");
        }
        assert_eq!(f.calls.get(), 0);
    }
    #[test]
    fn windows_requires_exact_fields() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let mut r = json!({"id":"1","method":"windows","pid":42});
        let (status, v) = execute(&r, &f).unwrap();
        assert_eq!(status, "ok");
        assert_eq!(v["windows"][0]["pid"], 42);
        r["unexpected"] = json!(true);
        assert!(execute(&r, &f).is_err());
        assert_eq!(f.calls.get(), 1);
    }
    #[test]
    fn all_windows_uses_separate_inventory_and_never_authorizes() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let request = json!({"id":"1","method":"all_windows","pid":42});
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["scope"], "all_window_candidates");
        assert_eq!(result["omitted_invalid_count"], 1);
        assert_eq!(result["action_allowed"], false);
        assert_eq!(result["windows"][0]["window_id"], 99);
        assert!(execute(&json!({"id":"1","method":"all_windows","pid":0}), &f).is_err());
        assert_eq!(f.calls.get(), 1);
    }
    #[test]
    fn subtree_validates_path_and_never_grants_authority() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let request = json!({"id":"s","method":"accessibility_subtree","pid":42,"ax_helper":"/helper","source":"main","path":[1,2]});
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["action_allowed"], false);
        assert_eq!(result["path"], json!([1, 2]));
        let mut invalid = request.clone();
        invalid["path"] = json!([160]);
        assert!(execute(&invalid, &f).is_err());
        invalid["path"] = json!([-1]);
        assert!(execute(&invalid, &f).is_err());
        invalid["path"] = json!([0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(execute(&invalid, &f).is_err());
        invalid["path"] = json!([]);
        invalid["source"] = "window".into();
        assert!(execute(&invalid, &f).is_err());
        invalid["source"] = "windows".into();
        assert!(execute(&invalid, &f).is_err());
        invalid["path"] = json!([32]);
        assert!(execute(&invalid, &f).is_err());
        invalid["path"] = json!([0, 2]);
        let (status, value) = execute(&invalid, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(value["path"], json!([0, 2]));
        assert_eq!(value["action_allowed"], false);
        assert_eq!(f.calls.get(), 2);
    }
    #[test]
    fn observed_all_windows_brackets_inventory_with_identity() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let request =
            json!({"id":"1","method":"observe_all_windows","pid":42,"ax_helper":"/helper"});
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["same_identity_samples"], true);
        assert_eq!(result["action_allowed"], false);
        assert_eq!(result["inventory"]["windows"][0]["window_id"], 99);
        assert_eq!(result["inventory"]["omitted_invalid_count"], 1);
        assert_eq!(f.calls.get(), 5);

        let changed = Fake {
            calls: Cell::new(0),
        };
        let (_, result) = execute(
            &json!({"id":"1","method":"observe_all_windows","pid":43,"ax_helper":"/helper"}),
            &changed,
        )
        .unwrap();
        assert_eq!(result["same_identity_samples"], false);
        assert!(result["inventory"].is_null());
        assert_eq!(changed.calls.get(), 2);
    }
    #[test]
    fn record_all_windows_preserves_incomplete_and_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("cu-record-windows-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let output = dir.join("record.json");
        let request = json!({"id":"1","method":"record_all_windows","pid":42,"ax_helper":"/helper","output":output});
        let f = Fake {
            calls: Cell::new(0),
        };
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "observation_incomplete");
        assert_eq!(result["observation_complete"], false);
        let bytes = std::fs::read(&output).unwrap();
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            saved["observation"]["inventory"]["omitted_invalid_count"],
            1
        );
        assert_eq!(saved["action_allowed"], false);
        let calls = f.calls.get();
        assert!(execute(&request, &f).is_err());
        assert_eq!(f.calls.get(), calls);
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        let failed = dir.join("failed.json");
        let f = Fake {
            calls: Cell::new(0),
        };
        let (status, _) = execute(&json!({"id":"1","method":"record_all_windows","pid":44,"ax_helper":"/helper","output":failed}),&f).unwrap();
        assert_eq!(status, "observation_incomplete");
        let saved: Value = serde_json::from_slice(&std::fs::read(&failed).unwrap()).unwrap();
        assert!(saved["error"]
            .as_str()
            .unwrap()
            .contains("invalid process identity"));
        assert_eq!(saved["action_allowed"], false);
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(failed).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn observed_all_windows_keeps_inventory_when_final_identity_fails() {
        struct Failing(Cell<u32>);
        impl Backend for Failing {
            fn identity(&self, pid: i32) -> Result<Value, String> {
                self.0.set(self.0.get() + 1);
                if self.0.get() > 2 {
                    return Err("unavailable".into());
                }
                Ok(
                    json!({"status":"ok","action_allowed":false,"pid":pid,"process_start_unix_seconds":123.0,"executable":"/test","bundle_id":"test.app"}),
                )
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
                Err("unused".into())
            }
            fn all_windows(&self, _: i32) -> Result<(Vec<Window>, usize), String> {
                Ok((vec![], 0))
            }
            fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
                Err("unused".into())
            }
        }
        let f = Failing(Cell::new(0));
        let (_, result) = execute(
            &json!({"id":"1","method":"observe_all_windows","pid":42,"ax_helper":"/helper"}),
            &f,
        )
        .unwrap();
        assert_eq!(result["same_identity_samples"], false);
        assert_eq!(result["inventory"]["scope"], "all_window_candidates");
        assert!(result["identity_after"].is_null());
        assert!(result["reason"]
            .as_str()
            .unwrap()
            .contains("final identity lookup failed"));
        assert_eq!(result["action_allowed"], false);
    }
    #[test]
    fn title_search_returns_all_exact_matches_without_authority() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let request = json!({"id":"1","method":"find_windows","title":"test"});
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["action_allowed"], false);
        assert_eq!(result["candidate_count"], 2);
        assert_eq!(result["candidates"][0]["pid"], 42);
        assert_eq!(result["candidates"][1]["pid"], 43);
        let (_, none) = execute(
            &json!({"id":"1","method":"find_windows","title":"Test"}),
            &f,
        )
        .unwrap();
        assert_eq!(none["candidate_count"], 0);
        assert!(execute(&json!({"id":"1","method":"find_windows","title":""}), &f).is_err());
        assert_eq!(f.calls.get(), 2);
    }
    #[test]
    fn process_identity_is_sampled_twice_and_never_authorizes() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let request = json!({"id":"1","method":"process_identity","pid":42,"ax_helper":"/helper"});
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["same_identity_samples"], true);
        assert_eq!(result["identity_before"]["bundle_id"], "test.app");
        assert_eq!(result["action_allowed"], false);
        assert_eq!(f.calls.get(), 2);
        assert!(execute(
            &json!({"id":"1","method":"process_identity","pid":0,"ax_helper":"/helper"}),
            &f
        )
        .is_err());
        assert_eq!(f.calls.get(), 2);
    }
    #[test]
    fn changed_or_unsafe_identity_never_looks_stable() {
        let f = Fake {
            calls: Cell::new(0),
        };
        let request = json!({"id":"1","method":"process_identity","pid":43,"ax_helper":"/helper"});
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["same_identity_samples"], false);
        assert_eq!(result["action_allowed"], false);
        assert!(execute(
            &json!({"id":"1","method":"process_identity","pid":44,"ax_helper":"/helper"}),
            &f
        )
        .is_err());
    }
    #[test]
    fn observed_all_windows_preserves_partial_final_identity() {
        struct FailsFourth(Cell<u32>);
        impl Backend for FailsFourth {
            fn identity(&self, pid: i32) -> Result<Value, String> {
                self.0.set(self.0.get() + 1);
                if self.0.get() == 4 {
                    return Err("unavailable".into());
                }
                Ok(
                    json!({"status":"ok","action_allowed":false,"pid":pid,"process_start_unix_seconds":123.0,"executable":"/test","bundle_id":"test.app"}),
                )
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
                Err("unused".into())
            }
            fn all_windows(&self, _: i32) -> Result<(Vec<Window>, usize), String> {
                Ok((vec![], 0))
            }
            fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
                Err("unused".into())
            }
        }
        let f = FailsFourth(Cell::new(0));
        let (_, result) = execute(
            &json!({"id":"1","method":"observe_all_windows","pid":42,"ax_helper":"/helper"}),
            &f,
        )
        .unwrap();
        assert_eq!(result["same_identity_samples"], false);
        assert_eq!(result["identity_after"]["identity_before"]["pid"], 42);
        assert!(result["identity_after"]["identity_after"].is_null());
        assert!(result["identity_after"]["reason"]
            .as_str()
            .unwrap()
            .contains("second process identity lookup failed"));
        assert_eq!(result["inventory"]["scope"], "all_window_candidates");
        assert_eq!(result["action_allowed"], false);
    }
    #[test]
    fn bundle_lookup_rejects_incomplete_or_unsafe_identity_rows() {
        assert!(valid_bundle_id("com.example.app"));
        assert!(!valid_bundle_id("com.example/other"));
        let row = json!({"pid":42,"bundle_id":"com.example.app","process_start_unix_seconds":123.0,"process_start_source":"kernel_sysctl_kern_proc","executable":"/app"});
        let mut value = json!({"status":"ok","action_allowed":false,"bundle_id":"com.example.app","incomplete_count":0,"candidates":[row]});
        assert!(valid_bundle_result(&value, "com.example.app"));
        value["candidates"][0]["process_start_source"] = "appkit_launch_date".into();
        assert!(!valid_bundle_result(&value, "com.example.app"));
        value["candidates"][0]
            .as_object_mut()
            .unwrap()
            .remove("process_start_source");
        assert!(!valid_bundle_result(&value, "com.example.app"));
        value["candidates"][0]["process_start_source"] = "kernel_sysctl_kern_proc".into();
        value["candidates"][0]["app_bundle_path"] = "/Applications/Example.app".into();
        assert!(valid_bundle_result(&value, "com.example.app"));
        value["candidates"][0]["app_bundle_path"] = "relative.app".into();
        assert!(!valid_bundle_result(&value, "com.example.app"));
        value["candidates"][0]["app_bundle_path"] = serde_json::Value::Null;
        value["candidates"][0]["pid"] = 0.into();
        assert!(!valid_bundle_result(&value, "com.example.app"));
        value["candidates"][0]["pid"] = 42.into();
        let duplicate = value["candidates"][0].clone();
        value["candidates"].as_array_mut().unwrap().push(duplicate);
        assert!(!valid_bundle_result(&value, "com.example.app"));
        value["candidates"].as_array_mut().unwrap().pop();
        value["action_allowed"] = true.into();
        assert!(!valid_bundle_result(&value, "com.example.app"));
    }
    #[test]
    fn bundle_window_search_keeps_candidates_without_authority() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cu-bundle-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        let response = json!({"status":"ok","action_allowed":false,"bundle_id":"test.app","incomplete_count":0,
            "candidates":[{"pid":42,"bundle_id":"test.app","process_start_unix_seconds":123.0,"process_start_source":"kernel_sysctl_kern_proc","executable":"/test"}]});
        std::fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", response)).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let f = Fake {
            calls: Cell::new(0),
        };
        let (status, result) = execute(&json!({"id":"1","method":"find_bundle_windows","bundle_id":"test.app","ax_helper":helper}), &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(result["action_allowed"], false);
        assert_eq!(result["same_complete_samples"], true);
        assert_eq!(result["windows"][0]["pid"], 42);
        assert_eq!(result["windows"][0]["window_id"], 9);
        assert_eq!(f.calls.get(), 1);
        let (status, all) = execute(&json!({"id":"2","method":"find_bundle_all_windows","bundle_id":"test.app","ax_helper":helper}), &f).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(all["action_allowed"], false);
        assert_eq!(all["same_complete_samples"], true);
        assert_eq!(all["all_inventory_calls_completed"], true);
        assert_eq!(all["inventories"][0]["omitted_invalid_count"], 1);
        assert_eq!(all["windows"][0]["window_id"], 99);
        assert_eq!(all["inventory_scope"], "all_window_candidates");
        std::fs::remove_file(helper).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn bundle_all_windows_keeps_partial_inventory_on_failure() {
        use std::os::unix::fs::PermissionsExt;
        struct Partial;
        impl Backend for Partial {
            fn identity(&self, _: i32) -> Result<Value, String> {
                Err("unused".into())
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
                Err("unused".into())
            }
            fn all_windows(&self, pid: i32) -> Result<(Vec<Window>, usize), String> {
                if pid == 43 {
                    return Err("inventory unavailable".into());
                }
                Ok((
                    vec![Window {
                        target: Target::new(pid, 99).unwrap(),
                        bounds: Rect {
                            x: 0.,
                            y: 0.,
                            width: 10.,
                            height: 10.,
                        },
                        layer: Some(0),
                        title: None,
                    }],
                    1,
                ))
            }
            fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
                Err("unused".into())
            }
        }
        let dir = std::env::temp_dir().join(format!("cu-bundle-partial-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        let row = |pid| json!({"pid":pid,"bundle_id":"test.app","process_start_unix_seconds":123.0,"process_start_source":"kernel_sysctl_kern_proc","executable":"/test"});
        let response = json!({"status":"ok","action_allowed":false,"bundle_id":"test.app","incomplete_count":0,"candidates":[row(42),row(43)]});
        std::fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", response)).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (status,v)=execute(&json!({"id":"partial","method":"find_bundle_all_windows","bundle_id":"test.app","ax_helper":helper}),&Partial).unwrap();
        assert_eq!(status, "unknown");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["all_inventory_calls_completed"], false);
        assert_eq!(v["same_complete_samples"], false);
        assert_eq!(v["windows"].as_array().unwrap().len(), 1);
        assert_eq!(v["inventories"][0]["omitted_invalid_count"], 1);
        assert!(v["after"].is_null());
        assert!(v["reason"].as_str().unwrap().contains("inventory failed"));
        std::fs::remove_file(helper).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn record_bundle_windows_preserves_partial_and_never_overwrites() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cu-record-bundle-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        let output = dir.join("record.json");
        let row = json!({"pid":42,"bundle_id":"test.app","process_start_unix_seconds":123.0,"process_start_source":"kernel_sysctl_kern_proc","executable":"/test"});
        let response = json!({"status":"ok","action_allowed":false,"bundle_id":"test.app","incomplete_count":0,"candidates":[row]});
        std::fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", response)).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let request = json!({"id":"record","method":"record_bundle_all_windows","bundle_id":"test.app","ax_helper":helper,"output":output});
        let f = Fake {
            calls: Cell::new(0),
        };
        let (status, result) = execute(&request, &f).unwrap();
        assert_eq!(status, "observation_incomplete");
        assert_eq!(result["query_complete"], false);
        let bytes = std::fs::read(&output).unwrap();
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            saved["observation"]["inventories"][0]["omitted_invalid_count"],
            1
        );
        assert_eq!(saved["action_allowed"], false);
        let count = f.calls.get();
        assert!(execute(&request, &f).is_err());
        assert_eq!(f.calls.get(), count);
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        let failed = dir.join("failed.json");
        std::fs::remove_file(&helper).unwrap();
        let (status,v)=execute(&json!({"id":"failed","method":"record_bundle_all_windows","bundle_id":"test.app","ax_helper":helper,"output":failed}),&f).unwrap();
        assert_eq!(status, "observation_incomplete");
        assert!(v["record"]["error"].is_string());
        assert!(
            serde_json::from_slice::<Value>(&std::fs::read(&failed).unwrap()).unwrap()["error"]
                .is_string()
        );
        std::fs::remove_file(output).unwrap();
        std::fs::remove_file(failed).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn failed_second_bundle_sample_keeps_first_and_stops_window_search() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("cu-bundle-failure-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        let marker = dir.join("called");
        let response = json!({"status":"ok","action_allowed":false,"bundle_id":"test.app","incomplete_count":0,
            "candidates":[{"pid":42,"bundle_id":"test.app","process_start_unix_seconds":123.0,"process_start_source":"kernel_sysctl_kern_proc","executable":"/test"}]});
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\nif [ -e '{}' ]; then exit 1; fi\ntouch '{}'\nprintf '%s' '{}'\n",
                marker.display(),
                marker.display(),
                response
            ),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let f = Fake {
            calls: Cell::new(0),
        };
        let (_, result) = execute(&json!({"id":"1","method":"find_bundle_windows","bundle_id":"test.app","ax_helper":helper}), &f).unwrap();
        assert_eq!(result["action_allowed"], false);
        assert_eq!(result["same_complete_samples"], false);
        assert_eq!(result["before"]["before"]["candidates"][0]["pid"], 42);
        assert!(result["before"]["after"].is_null());
        assert!(result["before"]["reason"]
            .as_str()
            .unwrap()
            .contains("second process sample failed"));
        assert_eq!(result["windows"], json!([]));
        assert_eq!(f.calls.get(), 0);
        std::fs::remove_file(marker).unwrap();
        std::fs::remove_file(helper).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn captured_frame_still_requires_agent_review() {
        let result = json!({"status":"captured","decision":"unknown","action_allowed":false});
        assert_eq!(capture_status(&result).unwrap(), "unknown");
        let failure = json!({"status":"error","decision":"unknown","action_allowed":false});
        assert_eq!(capture_status(&failure).unwrap(), "observation_incomplete");
        let unsafe_result = json!({"status":"captured","decision":"unknown","action_allowed":true});
        assert!(capture_status(&unsafe_result).is_err());
    }
}
