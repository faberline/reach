#![cfg(target_os = "macos")]
//! Launches `cu-mcp` with the real `cu-agent`. Every reply must keep `action_allowed` false.
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    io::Write,
    process::{Command, Stdio},
};

const SOCKET: (&str, &str) = ("REACH_AGENT_SOCKET", "/nonexistent/reach-agent.sock");

/// Every `action_allowed` key anywhere in `v` must be `false`.
fn never_allowed(v: &Value) {
    match v {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "action_allowed" {
                    assert_eq!(child, &Value::Bool(false), "{key}");
                }
                never_allowed(child);
            }
        }
        Value::Array(items) => items.iter().for_each(never_allowed),
        _ => {}
    }
}

/// Run `cu-mcp --cu-agent <cu-agent>` with the given request lines and parse each reply line.
fn session(lines: &[String]) -> Vec<Value> {
    session_with(&[], lines)
}

/// Like `session`, with `--helper <field>=<path>` for each entry of `helpers`.
fn session_with(helpers: &[&str], lines: &[String]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cu-mcp"))
        .args(["--cu-agent", env!("CARGO_BIN_EXE_cu-agent")])
        .args(helpers.iter().flat_map(|h| ["--helper", h]))
        .env(SOCKET.0, SOCKET.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for line in lines {
            stdin.write_all(line.as_bytes()).unwrap();
            stdin.write_all(b"\n").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let replies: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    replies.iter().for_each(never_allowed);
    replies
}

fn call(id: u64, name: &str, arguments: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
        .to_string()
}

fn direct_describe() -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cu-agent"))
        .env(SOCKET.0, SOCKET.1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"id":"d","method":"describe"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    never_allowed(&v);
    v
}

/// A field that `cu-agent` would execute as a program.
fn executable(field: &str) -> bool {
    field.ends_with("_helper") || field == "adapter" || field == "adapters"
}

/// Every field name a method declares, in any of its three lists, without the executable paths.
fn field_union(entry: &Value) -> BTreeSet<String> {
    ["fields", "agent_fields", "agent_optional_fields"]
        .iter()
        .filter_map(|key| entry[*key].as_array())
        .flatten()
        .map(|v| v.as_str().unwrap().to_string())
        .filter(|f| !executable(f))
        .collect()
}

/// The executable-path fields of one method.
fn executable_fields(entry: &Value) -> Vec<&str> {
    entry["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .filter(|f| executable(f))
        .collect()
}

fn list_tools(helpers: &[&str]) -> Vec<Value> {
    let replies = session_with(
        helpers,
        &[r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string()],
    );
    assert_eq!(replies.len(), 1);
    replies[0]["result"]["tools"].as_array().unwrap().clone()
}

#[test]
fn tools_match_the_describe_of_the_real_cu_agent() {
    let describe = direct_describe();
    let methods = describe["result"]["methods"].as_object().unwrap();
    let replies = session(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_string(),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string(),
    ]);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    let tools = replies[1]["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    // No helper was fixed: only methods that need none, or that ReachAgent can serve, are offered.
    let mut expected: Vec<&str> = methods
        .iter()
        .filter(|(_, e)| executable_fields(e).is_empty() || e.get("agent_fields").is_some())
        .map(|(name, _)| name.as_str())
        .collect();
    expected.sort();
    assert_eq!(names, expected);
    assert!(names.contains(&"action") && names.contains(&"describe"));
    assert!(names.contains(&"capture") && !names.contains(&"recognize_text"));
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let props: BTreeSet<String> = tool["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(props, field_union(&methods[name]), "{name}");
    }
}

#[test]
fn every_method_is_offered_once_its_helpers_are_fixed() {
    let describe = direct_describe();
    let methods = describe["result"]["methods"].as_object().unwrap();
    let mut fields: BTreeSet<&str> = methods.values().flat_map(executable_fields).collect();
    assert!(fields.contains("ax_helper") && fields.contains("adapters"));
    let helpers: Vec<String> = fields
        .iter()
        .map(|f| format!("{f}=/nonexistent/reach-{f}"))
        .collect();
    let helpers: Vec<&str> = helpers.iter().map(String::as_str).collect();
    let tools = list_tools(&helpers);
    let names: BTreeSet<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, methods.keys().map(String::as_str).collect());
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        let props: BTreeSet<String> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(props, field_union(&methods[name]), "{name}");
        for required in schema["required"].as_array().unwrap() {
            assert!(!executable(required.as_str().unwrap()), "{name}");
        }
    }

    // One helper less: exactly the methods that need it, and have no ReachAgent route, disappear.
    fields.remove("ax_helper");
    let helpers: Vec<String> = fields
        .iter()
        .map(|f| format!("{f}=/nonexistent/reach-{f}"))
        .collect();
    let helpers: Vec<&str> = helpers.iter().map(String::as_str).collect();
    let fewer = list_tools(&helpers);
    let names: BTreeSet<&str> = fewer.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(!names.contains("find_bundle") && !names.contains("observe_window"));
    assert!(names.contains("accessibility") && names.contains("recognize_text"));
}

#[test]
fn a_tool_argument_cannot_name_an_executable() {
    let image = "/nonexistent/reach-image.png";
    let asked = [
        call(
            1,
            "recognize_text",
            json!({"image": image, "ocr_helper": "/bin/sh"}),
        ),
        call(2, "recognize_text", json!({"image": image})),
        call(
            3,
            "recognize_image",
            json!({"image": image, "adapter": "/bin/sh"}),
        ),
        call(4, "windows", json!({"pid": 1, "ax_helper": "/bin/sh"})),
    ];

    // Without a fixed helper the argument is refused and the method is not run.
    let replies = session(&asked);
    assert_eq!(replies.len(), 4);
    for i in [0, 2, 3] {
        assert_eq!(replies[i]["error"]["code"], -32602, "{i}");
    }
    let s = &replies[1]["result"]["structuredContent"];
    assert_eq!(s["status"], "wrapper_error");
    assert!(s["reason"].as_str().unwrap().contains("ocr_helper"));
    assert!(s["reply"].is_null());

    // With a fixed helper the argument is still refused; the method runs with the fixed path.
    // The path does not exist, so `cu-agent` reports an error and nothing is executed.
    let replies = session_with(&["ocr_helper=/nonexistent/reach-ocr-helper"], &asked);
    assert_eq!(replies.len(), 4);
    for i in [0, 2, 3] {
        assert_eq!(replies[i]["error"]["code"], -32602, "{i}");
    }
    let s = &replies[1]["result"]["structuredContent"];
    assert_eq!(s["status"], "error");
    assert_eq!(s["reply"]["id"], "mcp-1");
    assert_eq!(s["reply"]["method"], "recognize_text");
    assert_ne!(
        s["reply"]["result"]["reason"],
        "missing or unexpected request fields"
    );
}

#[test]
fn describe_call_succeeds_through_cu_agent() {
    let replies = session(&[call(1, "describe", json!({}))]);
    assert_eq!(replies.len(), 1);
    let result = &replies[0]["result"];
    assert_eq!(result["isError"], false);
    let s = &result["structuredContent"];
    assert_eq!(s["status"], "ok");
    assert_eq!(s["exit_code"], 0);
    assert_eq!(s["needs_agent"], false);
    assert_eq!(s["reply"]["id"], "mcp-1");
    assert_eq!(
        s["reply"]["result"]["methods"]["action"]["availability"],
        "unsupported"
    );
}

#[test]
fn action_is_still_unsupported_through_mcp() {
    let replies = session(&[call(1, "action", json!({"event": {"kind": "click"}}))]);
    assert_eq!(replies.len(), 1);
    let result = &replies[0]["result"];
    assert_eq!(result["isError"], true);
    let s = &result["structuredContent"];
    assert_eq!(s["status"], "unsupported");
    assert_eq!(s["exit_code"], 3);
    assert_eq!(s["needs_agent"], true);
    assert_eq!(s["reply"]["action_allowed"], false);
}

#[test]
fn unknown_tool_and_invalid_arguments_are_cu_agent_errors() {
    let replies = session(&[
        call(1, "no_such_method", json!({})),
        call(2, "windows", json!({"pid": "x"})),
        call(3, "windows", json!({"pid": 1, "id": "z"})),
    ]);
    assert_eq!(replies.len(), 3);

    let first = &replies[0]["result"]["structuredContent"];
    assert_eq!(first["status"], "error");
    assert_eq!(replies[0]["result"]["isError"], true);
    assert_eq!(first["reply"]["result"]["reason"], "unknown method");
    assert_eq!(first["reply"]["id"], "mcp-1");

    let second = &replies[1]["result"]["structuredContent"];
    assert_eq!(second["status"], "error");
    assert_eq!(second["exit_code"], 1);
    assert_eq!(second["reply"]["id"], "mcp-2");

    assert_eq!(replies[2]["error"]["code"], -32602);
}

#[test]
fn oversized_line_is_refused_and_the_server_continues() {
    let replies = session(&[
        "a".repeat(65_537),
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#.to_string(),
    ]);
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["error"]["code"], -32600);
    assert!(replies[1].get("result").is_some());
}

#[test]
fn bad_arguments_exit_2_without_output() {
    let none = Command::new(env!("CARGO_BIN_EXE_cu-mcp")).output().unwrap();
    assert_eq!(none.status.code(), Some(2));
    assert!(none.stdout.is_empty());

    let relative = Command::new(env!("CARGO_BIN_EXE_cu-mcp"))
        .args(["--cu-agent", "relative/path"])
        .output()
        .unwrap();
    assert_eq!(relative.status.code(), Some(2));
    assert!(relative.stdout.is_empty());

    let bad_helpers: [&[&str]; 5] = [
        &["--helper"],
        &["--helper", "ax_helper=relative/ax"],
        &["--helper", "image=/nonexistent/a.png"],
        &["--ax-helper", "/nonexistent/ax"],
        &[
            "--helper",
            "ax_helper=/nonexistent/a",
            "--helper",
            "ax_helper=/nonexistent/b",
        ],
    ];
    for extra in bad_helpers {
        let out = Command::new(env!("CARGO_BIN_EXE_cu-mcp"))
            .args(["--cu-agent", env!("CARGO_BIN_EXE_cu-agent")])
            .args(extra)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{extra:?}");
        assert!(out.stdout.is_empty(), "{extra:?}");
    }
}
