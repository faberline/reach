use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
/// Hermetic by default: the agent socket points at a path where nothing can listen.
fn invoke(bytes: &[u8]) -> (i32, Value) {
    invoke_with_env(
        bytes,
        &[("REACH_AGENT_SOCKET", "/nonexistent/reach-agent.sock")],
    )
}
fn invoke_with_env(bytes: &[u8], env: &[(&str, &str)]) -> (i32, Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cu-agent"))
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.stderr.is_empty());
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}
fn tmpdir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "cu-agent-{}-{n}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn describe_and_invalid_requests_return_json() {
    let (code, v) = invoke(br#"{"id":"a","method":"describe"}"#);
    assert_eq!(code, 0);
    assert_eq!(v["id"], "a");
    assert_eq!(v["action_allowed"], false);
    assert_eq!(
        v["result"]["methods"]["action"]["availability"],
        "unsupported"
    );
    let (code, v) = invoke(br#"{"id":"a","method":"describe","bad":true}"#);
    assert_eq!(code, 1);
    assert_eq!(v["status"], "error");
    let (code, v) = invoke(&vec![b' '; 65_537]);
    assert_eq!(code, 1);
    assert_eq!(v["status"], "error");
}
#[test]
fn multiple_event_kinds_are_unvalidated_and_unsupported() {
    for kind in ["pointer_click", "pointer_move", "key_press"] {
        let r = json!({"id":"a","method":"action","event":{"kind":kind}});
        let (code, v) = invoke(r.to_string().as_bytes());
        assert_eq!(code, 3);
        assert_eq!(v["status"], "unsupported");
        assert_eq!(v["action_allowed"], false);
    }
}
#[test]
fn capture_failure_preserves_sidecar_and_is_not_success() {
    let d = tmpdir();
    let image = d.join("frame.png");
    let r = json!({"id":"a","method":"capture","pid":1,"window_id":1,"image":image,
 "capture_helper":"/no-such-capture","ax_helper":"/no-such-ax"});
    let (code, v) = invoke(r.to_string().as_bytes());
    assert_eq!(code, 1);
    assert_eq!(v["status"], "observation_incomplete");
    assert_eq!(v["action_allowed"], false);
    assert!(PathBuf::from(v["result"]["evidence_file"].as_str().unwrap()).is_file());
    assert!(!image.exists());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn duplicate_keys_are_rejected_at_any_depth() {
    let requests: &[&[u8]] = &[
        br#"{"id":"first","id":"second","method":"describe"}"#,
        br#"{"id":"a","method":"describe","meth\u006fd":"action"}"#,
        br#"{"id":"a","method":"action","event":{"kind":"pointer_move","kind":"pointer_click"}}"#,
        br#"{"id":"a","method":"action","event":{"kind":"x","nested":[{"x":1,"x":2}]}}"#,
    ];
    for request in requests {
        let (code, report) = invoke(request);
        assert_eq!(code, 1);
        assert_eq!(report["status"], "error");
        assert_eq!(report["action_allowed"], false);
        assert!(report["id"].is_null());
    }
    // The same key in different objects is not a duplicate.
    let (code, report) =
        invoke(br#"{"id":"a","method":"action","event":{"kind":"x","nested":{"kind":"y"}}}"#);
    assert_eq!(code, 3);
    assert_eq!(report["status"], "unsupported");
}

#[test]
fn ocr_refuses_invalid_paths_and_never_authorizes_input() {
    for request in [
        json!({"id":"ocr", "method":"recognize_text", "image":"relative.png", "ocr_helper":"/tmp/helper"}),
        json!({"id":"ocr", "method":"recognize_text", "image":"/tmp/missing-ocr-image.png", "ocr_helper":"/tmp/helper"}),
        json!({"id":"ocr", "method":"recognize_text", "image":"/tmp/missing-ocr-image.png", "ocr_helper":"/tmp/helper", "extra":1}),
    ] {
        let (code, response) = invoke(request.to_string().as_bytes());
        assert_eq!(code, 1);
        assert_eq!(response["action_allowed"], false);
    }
}

#[test]
fn simulated_input_is_offline_and_requires_balanced_sequence() {
    let req = json!({"id":"simulation", "method":"simulate_input", "width":100, "height":80,
        "events":[{"kind":"button_down","button":"left","x":10,"y":10},{"kind":"button_up","button":"left","x":10,"y":10}]});
    let (code, result) = invoke(req.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(result["action_allowed"], false);
    assert_eq!(result["result"]["status"], "synthetic_only");
    let bad = json!({"id":"simulation", "method":"simulate_input", "width":100, "height":80,
        "events":[{"kind":"button_up","button":"left","x":10,"y":10}]});
    let (code, result) = invoke(bad.to_string().as_bytes());
    assert_eq!(code, 1);
    assert_eq!(result["action_allowed"], false);
}

#[test]
fn native_preview_rejects_unbalanced_before_allocating() {
    let request = json!({"id":"native", "method":"preview_native_input", "width":100,"height":80,
        "events":[{"kind":"button_up","button":"left","x":10,"y":10}]});
    let (code, v) = invoke(request.to_string().as_bytes());
    assert_eq!(code, 1);
    assert_eq!(v["action_allowed"], false);
}

#[test]
#[ignore = "requires window server; constructs only, never posts"]
fn native_preview_through_cli_never_posts() {
    let request = json!({"id":"native", "method":"preview_native_input", "width":100,"height":80,
        "events":[{"kind":"move","x":10,"y":10},{"kind":"button_down","button":"left","x":10,"y":10},{"kind":"move","x":20,"y":20},{"kind":"button_up","button":"left","x":20,"y":20}]});
    let (code, v) = invoke(request.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(v["action_allowed"], false);
    assert_eq!(v["result"]["status"], "native_memory_only");
    assert_eq!(
        v["result"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["native_type"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![5, 1, 6, 2]
    );
    let direct = json!({"id":"direct-click","method":"preview_native_input","width":100,"height":80,"events":[
        {"kind":"button_down","button":"left","x":10,"y":10},{"kind":"button_up","button":"left","x":10,"y":10}]});
    let (code, v) = invoke(direct.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(v["action_allowed"], false);
    assert_eq!(
        v["result"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["native_type"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    let chord = json!({"id":"chord","method":"preview_native_input","width":100,"height":80,"events":[
        {"kind":"key_down","keycode":56,"x":10,"y":10},{"kind":"key_down","keycode":0,"x":10,"y":10},
        {"kind":"key_up","keycode":0,"x":10,"y":10},{"kind":"key_up","keycode":56,"x":10,"y":10}]});
    let (code, v) = invoke(chord.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(v["action_allowed"], false);
    let events = v["result"]["events"].as_array().unwrap();
    assert_eq!(
        events
            .iter()
            .map(|x| x["native_type"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![12, 10, 11, 12]
    );
    assert_eq!(
        events
            .iter()
            .map(|x| x["native_modifier_flags"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1 << 17, 1 << 17, 1 << 17, 0]
    );
}

#[test]
fn tagged_preview_rejects_invalid_requests() {
    for (tag, events) in [
        (0, json!([{"kind":"move","x":10,"y":10}])),
        (
            1,
            json!([{"kind":"key_down","keycode":0,"x":10,"y":10},{"kind":"key_up","keycode":0,"x":10,"y":10}]),
        ),
        (
            1,
            json!([{"kind":"button_up","button":"left","x":10,"y":10}]),
        ),
    ] {
        let request = json!({"id":"tagged", "method":"preview_tagged_input", "width":100,"height":80,"synthetic_window_id":tag,"events":events});
        let (code, v) = invoke(request.to_string().as_bytes());
        assert_eq!(code, 1);
        assert_eq!(v["action_allowed"], false);
    }
}

#[test]
#[ignore = "requires window server; constructs only, never posts"]
fn tagged_preview_through_cli_never_posts() {
    let request = json!({"id":"tagged", "method":"preview_tagged_input", "width":100,"height":80,"synthetic_window_id":1193046,
        "events":[{"kind":"button_down","button":"left","x":10,"y":10},{"kind":"button_up","button":"left","x":10,"y":10}]});
    let (code, v) = invoke(request.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(v["action_allowed"], false);
    assert_eq!(v["result"]["status"], "tagged_native_memory_only");
    for event in v["result"]["events"].as_array().unwrap() {
        assert_eq!(event["synthetic_window_fields"]["field_91"], 1193046);
        assert_eq!(event["synthetic_window_fields"]["field_92"], 1193046);
    }
}

#[cfg(unix)]
#[test]
fn isolation_unknown_is_preserved_without_authorizing_input() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir();
    let helper = d.join("helper");
    fs::write(&helper,"#!/bin/sh\nprintf '%s' '{\"status\":\"unknown\",\"reason\":\"focusUnavailable\",\"action_allowed\":false,\"input_posted\":false}'\nexit 1\n").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let req = json!({"id":"test","method":"sample_isolation","isolation_helper":helper});
    let (code, v) = invoke(req.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(v["status"], "unknown");
    assert_eq!(v["result"]["reason"], "focusUnavailable");
    assert_eq!(v["action_allowed"], false);
    fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
#[test]
fn isolation_partial_does_not_become_full_proof() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir();
    let helper = d.join("helper");
    let result = json!({"status":"sampled_partial","focus_scope":"front_app_only","system_focus_sampled":false,"timing_within_limit":true,"action_allowed":false,"input_posted":false,"samples":10,"foreground_unchanged":true,"focus_unchanged":true,"cursor_unchanged":true,"window_order_unchanged":true});
    fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", result)).unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let req = json!({"id":"test","method":"sample_isolation","isolation_helper":helper});
    let (code, v) = invoke(req.to_string().as_bytes());
    assert_eq!(code, 3);
    assert_eq!(v["action_allowed"], false);
    assert_eq!(v["result"]["status"], "sampled_partial");
    assert_eq!(v["result"]["system_focus_sampled"], false);
    fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
#[test]
fn isolation_mixed_focus_is_unknown_and_bad_claims_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir();
    let helper = d.join("helper");
    let mut value = json!({"status":"sampled_partial","focus_scope":"mixed","system_focus_sampled":false,"timing_within_limit":true,"action_allowed":false,"input_posted":false,"samples":10,"foreground_unchanged":true,"focus_unchanged":null,"cursor_unchanged":true,"window_order_unchanged":true});
    let run = |value: &Value| {
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", value)).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let req = json!({"id":"x","method":"sample_isolation","isolation_helper":helper});
        invoke(req.to_string().as_bytes())
    };
    let (code, v) = run(&value);
    assert_eq!(code, 3);
    assert!(v["result"]["focus_unchanged"].is_null());
    value["focus_unchanged"] = true.into();
    assert_eq!(run(&value).0, 1);
    value["focus_scope"] = "system".into();
    value["system_focus_sampled"] = true.into();
    value["timing_within_limit"] = false.into();
    assert_eq!(run(&value).0, 3);
    value["status"] = "sampled".into();
    assert_eq!(run(&value).0, 1);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn combined_observation_preserves_failure_and_refuses_overwrite() {
    let d = tmpdir();
    let image = d.join("frame.png");
    let req = json!({"id":"joint","method":"observe_window","pid":1,"window_id":1,"image":image,
      "capture_helper":"/no-such-capture","ax_helper":"/no-such-ax"});
    let (code, v) = invoke(req.to_string().as_bytes());
    assert_eq!(code, 1);
    assert_eq!(v["status"], "observation_incomplete");
    assert_eq!(v["action_allowed"], false);
    assert_eq!(v["result"]["consistency"], "incomplete");
    let record = d.join("frame.png.binding.json");
    assert!(record.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&record).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    let before = fs::read(&record).unwrap();
    let (code, v) = invoke(req.to_string().as_bytes());
    assert_eq!(code, 1);
    assert_eq!(v["action_allowed"], false);
    assert_eq!(fs::read(&record).unwrap(), before);
    fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
#[test]
fn focus_notifications_are_review_only_and_malformed_reports_stop() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir();
    let helper = d.join("notify-helper");
    let mut report = json!({"status":"sampled","action_allowed":false,"input_posted":false,
        "registration":{"focused_element":true,"focused_window":true,"deactivated":true},
        "counts":{"focused_element":0,"focused_window":0,"deactivated":1},
        "frontmost_same_at_end":true,"foreground_identity":{"pid":10,"started_seconds":123,"started_microseconds":4},"foreground_identity_at_end":{"pid":10,"started_seconds":123,"started_microseconds":4},"duration_seconds":1.1});
    let run = |value: &Value| {
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", value)).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let req = json!({"id":"notify","method":"sample_focus_notifications","notification_helper":helper});
        invoke(req.to_string().as_bytes())
    };
    let (code, value) = run(&report);
    assert_eq!(code, 3);
    assert_eq!(value["action_allowed"], false);
    assert_eq!(value["result"]["counts"]["deactivated"], 1);
    report["registration"]["deactivated"] = false.into();
    assert_eq!(run(&report).0, 1);
    report["status"] = "sampled_partial".into();
    assert_eq!(run(&report).0, 3);
    report["input_posted"] = true.into();
    assert_eq!(run(&report).0, 1);
    fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
#[test]
fn session_status_rejects_unsafe_or_missing_flags() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmpdir();
    let helper = d.join("session-helper");
    for (body, code) in [
        (
            r#"{"status":"unknown","action_allowed":false,"input_posted":false,"dictionary_available":true,"on_console":true,"login_done":true,"note":"unverified"}"#,
            3,
        ),
        (
            r#"{"status":"unknown","action_allowed":false,"input_posted":false,"dictionary_available":false,"on_console":true,"login_done":null,"note":"unverified"}"#,
            1,
        ),
        (
            r#"{"status":"unknown","action_allowed":true,"input_posted":false,"dictionary_available":true,"on_console":true,"login_done":true,"note":"unverified"}"#,
            1,
        ),
        (
            r#"{"status":"unknown","action_allowed":false,"input_posted":false,"dictionary_available":true,"other":null,"login_done":true,"note":"unverified"}"#,
            1,
        ),
    ] {
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s\\n' '{body}'\n")).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let request = json!({"id":"session","method":"session_status","session_helper":helper});
        let (actual, result) = invoke(request.to_string().as_bytes());
        assert_eq!(actual, code);
        assert_eq!(result["action_allowed"], false);
    }
    fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
mod agent_route {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        os::unix::net::UnixListener,
        path::Path,
        thread::JoinHandle,
    };

    // A valid 1×1 RGBA PNG and its SHA-256.
    pub(super) const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60,
        0x60, 0x60, 0xf8, 0x0f, 0x00, 0x01, 0x04, 0x01, 0x00, 0x5f, 0xe5, 0xc3, 0x4b, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    pub(super) const PNG_SHA256: &str =
        "abc58d5127d7cdf313beb9ec8ee839860a9c6bfbc48c8b8eb6a3f7d8bb63de6f";

    /// A short directory: socket paths are limited to 100 bytes.
    fn short_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "ra{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        p
    }

    /// Serves one reply per entry, in order, and returns the request lines it saw.
    fn serve(socket: &Path, replies: Vec<Value>) -> JoinHandle<Vec<Value>> {
        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(&stream).read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                let mut reply = reply;
                reply["id"] = request["id"].clone();
                stream.write_all(format!("{reply}\n").as_bytes()).unwrap();
                seen.push(request);
            }
            seen
        })
    }

    fn ok(result: Value) -> Value {
        json!({"version": 1, "id": null, "ok": true, "result": result})
    }

    fn accessibility_report() -> Value {
        json!({
            "schema_version": 1, "status": "ok", "pid": 42,
            "process_start_unix_seconds": 1.0,
            "process_start_source": "kernel_sysctl_kern_proc",
            "bundle_id": null, "executable": null,
            "scope": "process_ax_windows", "tree_scope": "full_bounded_tree",
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

    fn capture_report(image: &str) -> Value {
        json!({
            "window_id": 7, "owner_pid": 42,
            "window_frame_points": {"x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0},
            "filter_content_rect_points": {"x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0},
            "point_pixel_scale": 1.0, "image_width": 2, "image_height": 2,
            "capture_start_uptime_ns": 1, "capture_end_uptime_ns": 2,
            "capture_end_unix_seconds": 0.0, "image_path": image,
            "capture_type": "desktop_independent_window",
            "window_scope": "onscreen_candidates",
            "onscreen_before": true, "onscreen_after": true, "occlusion_checked": false
        })
    }

    fn with_socket(socket: &Path, request: &Value) -> (i32, Value) {
        invoke_with_env(
            request.to_string().as_bytes(),
            &[("REACH_AGENT_SOCKET", socket.to_str().unwrap())],
        )
    }

    #[test]
    fn agent_route_forwards_accessibility_to_the_socket() {
        let d = short_dir();
        let socket = d.join("s.sock");
        let server = serve(&socket, vec![ok(accessibility_report())]);
        let request = json!({"id":"a1","method":"accessibility","pid":42});
        let (code, v) = with_socket(&socket, &request);
        assert_eq!(code, 0);
        assert_eq!(v["protocol"], "mac-computer-use-agent-v1");
        assert_eq!(v["id"], "a1");
        assert_eq!(v["method"], "accessibility");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["result"], accessibility_report());
        let seen = server.join().unwrap();
        assert_eq!(seen[0]["method"], "accessibility");
        assert_eq!(seen[0]["params"], json!({"pid": 42}));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn agent_route_capture_adds_the_image_hash() {
        let d = short_dir();
        let socket = d.join("s.sock");
        let image = d.join("frame.png");
        fs::write(&image, PNG).unwrap();
        let image_text = image.to_str().unwrap();
        let server = serve(&socket, vec![ok(capture_report(image_text))]);
        let request =
            json!({"id":"c1","method":"capture","pid":42,"window_id":7,"image":image_text});
        let (code, v) = with_socket(&socket, &request);
        assert_eq!(code, 3);
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["result"]["image_sha256"], PNG_SHA256);
        let mut report = v["result"].clone();
        report.as_object_mut().unwrap().remove("image_sha256");
        assert_eq!(report, capture_report(image_text));
        server.join().unwrap();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn agent_route_without_the_app_is_unavailable() {
        let d = short_dir();
        let socket = d.join("s.sock");
        let image = d.join("frame.png");
        let image_text = image.to_str().unwrap();
        let requests = [
            json!({"id":"u1","method":"accessibility","pid":42}),
            json!({"id":"u2","method":"capture","pid":42,"window_id":7,"image":image_text}),
            json!({"id":"u3","method":"accessibility_subtree","pid":42,
                   "source":"main","path":[0]}),
        ];
        for request in requests {
            let (code, v) = with_socket(&socket, &request);
            assert_eq!(code, 3, "{request}");
            assert_eq!(v["status"], "agent_unavailable");
            assert_eq!(v["action_allowed"], false);
            assert_eq!(v["result"]["fallback"], "none");
            assert_eq!(v["result"]["socket"], socket.to_str().unwrap());
            assert!(v["result"]["start"]
                .as_str()
                .unwrap()
                .starts_with("open -a ReachAgent"));
        }
        assert!(!socket.exists(), "cu-agent must not create the socket");
        assert!(!image.exists(), "no image without the App");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn agent_route_with_an_invalid_socket_path_is_an_error() {
        let request = json!({"id":"p1","method":"accessibility","pid":42});
        let long = format!("/{}", "a".repeat(120));
        for path in ["relative.sock", long.as_str()] {
            let (code, v) = invoke_with_env(
                request.to_string().as_bytes(),
                &[("REACH_AGENT_SOCKET", path)],
            );
            assert_eq!(code, 1, "{path}");
            assert_eq!(v["status"], "error");
            assert_eq!(v["action_allowed"], false);
            assert!(v["result"]["reason"].is_string());
            assert!(v["result"].get("start").is_none());
        }
    }

    #[test]
    fn agent_route_reports_service_errors_as_errors() {
        let d = short_dir();
        let socket = d.join("s.sock");
        let denied = json!({"version": 1, "id": null, "ok": false, "error": {
            "code": "permission_denied", "message": "denied",
            "permission": "accessibility", "state": "denied"}});
        let server = serve(&socket, vec![denied]);
        let request = json!({"id":"e1","method":"accessibility","pid":42});
        let (code, v) = with_socket(&socket, &request);
        assert_eq!(code, 1);
        assert_eq!(v["status"], "error");
        assert_eq!(v["result"]["agent_error"]["code"], "permission_denied");
        assert_eq!(v["result"]["agent_error"]["permission"], "accessibility");
        server.join().unwrap();
        // A bad request is refused before any connection.
        let (code, v) = with_socket(
            &socket,
            &json!({"id":"e2","method":"accessibility","pid":0}),
        );
        assert_eq!(code, 1);
        assert_eq!(v["status"], "error");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn legacy_helper_requests_never_touch_the_socket() {
        let d = short_dir();
        let socket = d.join("s.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let request = json!({"id":"l1","method":"accessibility","pid":1,
            "ax_helper":"/no-such-ax"});
        let (code, v) = with_socket(&socket, &request);
        assert_eq!(code, 1);
        assert_eq!(v["status"], "error");
        assert!(v["result"]["reason"].is_string());
        let err = listener.accept().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn describe_reports_the_agent_state() {
        let d = short_dir();
        let socket = d.join("s.sock");
        let (code, v) = with_socket(&socket, &json!({"id":"d1","method":"describe"}));
        assert_eq!(code, 0);
        assert_eq!(v["result"]["agent"]["state"], "unavailable");
        let methods = &v["result"]["methods"];
        assert_eq!(
            methods["capture"]["agent_fields"],
            json!(["pid", "window_id", "image"])
        );
        assert_eq!(
            methods["accessibility"]["agent_optional_fields"],
            json!(["windows_only"])
        );
        let legacy = [
            (
                "capture",
                json!(["pid", "window_id", "image", "capture_helper", "ax_helper"]),
            ),
            (
                "capture_all_windows",
                json!(["pid", "window_id", "image", "capture_helper", "ax_helper"]),
            ),
            ("accessibility", json!(["pid", "ax_helper"])),
            (
                "accessibility_subtree",
                json!(["pid", "ax_helper", "source", "path"]),
            ),
        ];
        let agent = [
            ("capture", json!(["pid", "window_id", "image"])),
            ("capture_all_windows", json!(["pid", "window_id", "image"])),
            ("accessibility", json!(["pid"])),
            ("accessibility_subtree", json!(["pid", "source", "path"])),
        ];
        for ((method, fields), (_, agent_fields)) in legacy.iter().zip(agent.iter()) {
            assert_eq!(&methods[*method]["fields"], fields, "{method}");
            assert_eq!(&methods[*method]["agent_fields"], agent_fields, "{method}");
        }
        let describe = ok(json!({"app": "ReachAgent", "app_version": "0.1.0",
            "protocol_version": 1, "methods": [], "test_mode": true}));
        let permissions = ok(json!({"accessibility": "granted",
            "screen_recording": "unknown", "post_event": "unknown"}));
        let server = serve(&socket, vec![describe, permissions]);
        let (code, v) = with_socket(&socket, &json!({"id":"d2","method":"describe"}));
        assert_eq!(code, 0);
        assert_eq!(v["result"]["agent"]["state"], "running");
        assert_eq!(
            v["result"]["agent"]["permissions"]["accessibility"],
            "granted"
        );
        let seen = server.join().unwrap();
        let names: Vec<&str> = seen.iter().map(|r| r["method"].as_str().unwrap()).collect();
        assert_eq!(names, ["describe", "permissions"]);
        fs::remove_dir_all(d).unwrap();
    }
}

#[cfg(target_os = "macos")]
mod interpret {
    use super::{
        agent_route::{PNG, PNG_SHA256},
        *,
    };
    use std::{os::unix::fs::PermissionsExt, path::Path};

    /// Writes a POSIX shell helper that runs `body` and is executable only by its owner.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }
    /// An adapter that prints `value` as its only output. The JSON must not contain single quotes.
    fn adapter(dir: &Path, name: &str, value: &Value) -> PathBuf {
        script(dir, name, &format!("printf '%s' '{value}'"))
    }
    /// A well-formed `recognize_image` result for the fixture PNG, with an interpreter and answer.
    fn good(name: &str, answer: Value) -> Value {
        json!({"status":"review_required","action_allowed":false,"image_sha256":PNG_SHA256,
            "image_width":1,"image_height":1,
            "interpreter":{"name":name,"version":"1.0"},"answer":answer})
    }
    fn request(image: &Path, adapters: &[&Path], output: &Path) -> Value {
        let adapters: Vec<String> = adapters.iter().map(|p| p.display().to_string()).collect();
        json!({"id":"i1","method":"interpret_image",
            "image":image.display().to_string(),"adapters":adapters,
            "output":output.display().to_string()})
    }

    #[test]
    fn interpret_two_agreeing_adapters_records_evidence() {
        let dir = tmpdir();
        fs::write(dir.join("frame.png"), PNG).unwrap();
        let image = dir.join("frame.png");
        let output = dir.join("evidence.json");
        let mut first = good("alpha", json!("victory"));
        first["score"] = json!(0.9);
        let a = adapter(&dir, "a", &first);
        let b = adapter(&dir, "b", &good("beta", json!("victory")));
        let (code, v) = invoke(request(&image, &[&a, &b], &output).to_string().as_bytes());
        assert_eq!(code, 3);
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["action_allowed"], false);
        let result = &v["result"];
        assert_eq!(result["agreement"], "agree");
        assert_eq!(result["interpretation_complete"], true);
        assert_eq!(result["evidence_file"], output.display().to_string());
        let on_disk: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(on_disk, result["record"]);
        let record = &on_disk;
        assert_eq!(record["schema"], "cu-interpretation-evidence-v1");
        assert_eq!(record["action_allowed"], false);
        assert_eq!(record["image_sha256"], PNG_SHA256);
        assert_eq!(record["image_stable"], true);
        assert_eq!(record["interpretations"][0]["raw"]["score"], 0.9);
        assert_eq!(
            record["interpretations"][0]["interpreter"]["version"],
            "1.0"
        );
        let digest = record["interpretations"][0]["adapter_sha256"]
            .as_str()
            .unwrap();
        assert_eq!(digest.len(), 64);
        let mode = fs::metadata(&output).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interpret_disagreement_is_kept_and_never_authorizes() {
        let dir = tmpdir();
        fs::write(dir.join("frame.png"), PNG).unwrap();
        let image = dir.join("frame.png");
        let output = dir.join("evidence.json");
        let a = adapter(&dir, "a", &good("alpha", json!("victory")));
        let b = adapter(&dir, "b", &good("beta", json!("defeat")));
        let (code, v) = invoke(request(&image, &[&a, &b], &output).to_string().as_bytes());
        assert_eq!(code, 3);
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["result"]["agreement"], "disagree");
        assert_eq!(
            v["result"]["record"]["interpretations"][0]["answer"],
            "victory"
        );
        assert_eq!(
            v["result"]["record"]["interpretations"][1]["answer"],
            "defeat"
        );
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["result"]["action_allowed"], false);
        assert_eq!(v["result"]["record"]["action_allowed"], false);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interpret_failure_abstention_and_bad_format_are_incomplete() {
        let dir = tmpdir();
        fs::write(dir.join("frame.png"), PNG).unwrap();
        let image = dir.join("frame.png");
        let output = dir.join("evidence.json");
        let failing = script(&dir, "fail", "exit 1");
        let abstaining = adapter(&dir, "abstain", &good("alpha", Value::Null));
        let mut missing_interpreter = good("beta", json!("victory"));
        missing_interpreter
            .as_object_mut()
            .unwrap()
            .remove("interpreter");
        let bad_format = adapter(&dir, "bad", &missing_interpreter);
        let not_json = script(&dir, "junk", "printf '%s' 'not json'");
        let (code, v) = invoke(
            request(
                &image,
                &[&failing, &abstaining, &bad_format, &not_json],
                &output,
            )
            .to_string()
            .as_bytes(),
        );
        assert_eq!(code, 1);
        assert_eq!(v["status"], "interpretation_incomplete");
        assert_eq!(v["action_allowed"], false);
        let result = &v["result"];
        let entries = result["record"]["interpretations"].as_array().unwrap();
        let outcomes: Vec<&str> = entries
            .iter()
            .map(|e| e["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(
            outcomes,
            ["failed", "abstained", "invalid_format", "failed"]
        );
        assert_eq!(result["agreement"], "incomplete");
        assert!(entries[0]["raw"].is_null());
        assert!(entries[0]["reason"].as_str().is_some_and(|r| !r.is_empty()));
        assert_eq!(entries[2]["raw"]["answer"], "victory");
        let on_disk: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(on_disk, result["record"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interpret_image_change_is_detected() {
        let dir = tmpdir();
        fs::write(dir.join("frame.png"), PNG).unwrap();
        let image = dir.join("frame.png");
        let output = dir.join("evidence.json");
        let changing = script(
            &dir,
            "changing",
            &format!(
                "printf x >> \"$1\"; printf '%s' '{}'",
                good("alpha", json!("victory"))
            ),
        );
        let (code, v) = invoke(
            request(&image, &[&changing], &output)
                .to_string()
                .as_bytes(),
        );
        assert_eq!(code, 1);
        assert_eq!(v["status"], "interpretation_incomplete");
        let record = &v["result"]["record"];
        assert_eq!(record["image_stable"], false);
        assert_eq!(record["interpretations"][0]["outcome"], "failed");
        assert_eq!(record["image_sha256"], PNG_SHA256);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interpret_refuses_overwrite_and_invalid_requests() {
        let dir = tmpdir();
        fs::write(dir.join("frame.png"), PNG).unwrap();
        let image = dir.join("frame.png");
        let output = dir.join("evidence.json");
        let marker = dir.join("marker");

        // An existing evidence file is never overwritten, and the adapter never runs.
        fs::write(&output, b"keep").unwrap();
        let touch = script(&dir, "touch", &format!(": > {}", marker.display()));
        let (code, v) = invoke(request(&image, &[&touch], &output).to_string().as_bytes());
        assert_eq!(code, 1);
        assert_eq!(v["status"], "error");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(fs::read(&output).unwrap(), b"keep");
        assert!(!marker.exists());
        fs::remove_dir_all(&dir).unwrap();
        fs::create_dir(&dir).unwrap();
        // The image must exist again, or every case below would fail on the image alone.
        fs::write(&image, PNG).unwrap();

        let image_s = image.display().to_string();
        let output_s = output.display().to_string();
        let adapters: Vec<String> = (0..5)
            .map(|i| {
                adapter(&dir, &format!("p{i}"), &good("a", json!("victory")))
                    .display()
                    .to_string()
            })
            .collect();
        let one = adapters[0].clone();
        let req = |image: Value, adapters: Value| {
            json!({"id":"i1","method":"interpret_image","image":image,
                "adapters":adapters,"output":output_s})
        };
        // With a valid image, only the adapter rules can refuse these requests.
        for bad in [
            json!([]),
            json!(adapters.clone()),
            json!([one, one]),
            json!(["relative"]),
            json!([1]),
        ] {
            let case = req(json!(image_s), bad);
            let (code, v) = invoke(case.to_string().as_bytes());
            assert_eq!(code, 1, "{case}");
            assert_eq!(v["status"], "error", "{case}");
            assert_eq!(v["action_allowed"], false, "{case}");
            assert!(v.to_string().contains("invalid adapters"), "{case}: {v}");
            assert!(!output.exists(), "{case}");
        }
        let mut cases = vec![
            req(json!("relative.png"), json!([one])),
            req(
                json!(dir.join("missing.png").display().to_string()),
                json!([one]),
            ),
        ];
        let mut extra = req(json!(image_s), json!([one]));
        extra["extra"] = json!(1);
        cases.push(extra);
        for case in cases {
            let (code, v) = invoke(case.to_string().as_bytes());
            assert_eq!(code, 1, "{case}");
            assert_eq!(v["status"], "error", "{case}");
            assert_eq!(v["action_allowed"], false, "{case}");
            assert!(!output.exists(), "{case}");
        }
        fs::remove_dir_all(dir).unwrap();
    }
}

mod overlay {
    use super::*;
    use std::{os::unix::fs::PermissionsExt, path::Path};

    /// Writes a fake overlay helper that answers like the real one. It is owner-executable only.
    fn helper(dir: &Path) -> PathBuf {
        let p = dir.join("fake-overlay");
        let script = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":"\([^"]*\)".*/\1/')
  case "$line" in
    *'"quit"'*) printf '{"protocol":"cursor-overlay-v1","id":"%s","status":"ok","reason":null,"visible":false,"phase":"idle","point":null,"window_binding":"unbound","action_allowed":false}\n' "$id"; exit 0;;
    *) printf '{"protocol":"cursor-overlay-v1","id":"%s","status":"ok","reason":null,"visible":true,"phase":"idle","point":{"space":"global_display_points_top_left","display_id":1,"x":10,"y":20},"window_binding":"unbound","action_allowed":false}\n' "$id";;
  esac
done
"#;
        fs::write(&p, script).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }

    fn request(helper: &Path, timeout_ms: Value, steps: Value) -> Value {
        json!({"id":"o1","method":"overlay_session",
            "overlay_helper":helper.display().to_string(),
            "timeout_ms":timeout_ms,"steps":steps})
    }

    #[test]
    fn overlay_session_is_described() {
        let (code, v) = invoke(br#"{"id":"a","method":"describe"}"#);
        assert_eq!(code, 0);
        assert_eq!(
            v["result"]["methods"]["overlay_session"]["fields"],
            json!(["overlay_helper", "timeout_ms", "steps"])
        );
    }

    #[test]
    fn overlay_session_runs_a_fake_helper() {
        let d = tmpdir();
        let h = helper(&d);
        let steps = json!([
            {"command":"show","display_id":1,"x":10,"y":20},
            {"command":"query"}
        ]);
        let (code, v) = invoke(request(&h, json!(30000), steps).to_string().as_bytes());
        assert_eq!(code, 3, "{v}");
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["result"]["action_allowed"], false);
        assert_eq!(v["result"]["completed"], true);
        assert_eq!(v["result"]["steps"].as_array().unwrap().len(), 2);
        assert_eq!(v["result"]["steps"][1]["state"]["visible"], true);
        assert_eq!(v["result"]["cleanup"], "quit_acknowledged");
        assert_eq!(v["result"]["overlay_after"], "cleared");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn overlay_session_reports_failure_and_refuses_invalid_requests() {
        let d = tmpdir();
        let h = helper(&d);
        let steps = json!([{"command":"show","display_id":1,"x":10,"y":20}]);

        let missing = d.join("missing-helper");
        let (code, v) = invoke(
            request(&missing, json!(30000), steps.clone())
                .to_string()
                .as_bytes(),
        );
        assert_eq!(code, 1, "{v}");
        assert_eq!(v["status"], "overlay_incomplete");
        assert_eq!(v["result"]["stop"]["reason"], "helper_unavailable");
        assert_eq!(v["result"]["last_reported_state"], "unknown");
        assert_eq!(v["action_allowed"], false);

        let mut cases = Vec::new();
        let mut relative = request(&h, json!(30000), steps.clone());
        relative["overlay_helper"] = json!("helper");
        cases.push(relative);
        cases.push(request(&h, json!(99), steps.clone()));
        cases.push(request(&h, json!(60001), steps.clone()));
        cases.push(request(&h, json!(30000), json!([])));
        cases.push(request(&h, json!(30000), json!([{"command":"quit"}])));
        let mut extra = request(&h, json!(30000), steps.clone());
        extra["extra"] = json!(1);
        cases.push(extra);
        let mut no_timeout = request(&h, json!(30000), steps.clone());
        no_timeout.as_object_mut().unwrap().remove("timeout_ms");
        cases.push(no_timeout);
        for case in cases {
            let (code, v) = invoke(case.to_string().as_bytes());
            assert_eq!(code, 1, "{case}");
            assert_eq!(v["status"], "error", "{case}");
            assert_eq!(v["action_allowed"], false, "{case}");
        }
        fs::remove_dir_all(d).unwrap();
    }
}
