//! MCP stdio server. Every tool call runs one `cu-agent` process and returns its reply.
//! It never implements capture, accessibility, or input.
use crate::strict_json;
use serde_json::{json, Map, Value};
use std::{
    io::{self, BufRead, Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const MAX_LINE: usize = 65_536;
pub const MAX_REPLY: u64 = 8 * 1024 * 1024;
pub const CALL_TIMEOUT: Duration = Duration::from_secs(180);

pub struct AgentOutput {
    pub stdout: Vec<u8>,
    pub exit_code: Option<i32>,
}

pub struct Server<R: FnMut(&[u8]) -> Result<AgentOutput, String>> {
    run: R,
    calls: u64,
}

impl<R: FnMut(&[u8]) -> Result<AgentOutput, String>> Server<R> {
    pub fn new(run: R) -> Self {
        Self { run, calls: 0 }
    }

    /// 處理一行。通知與空行回傳 `None`。
    pub fn handle_line(&mut self, line: &[u8]) -> Option<Value> {
        let end = line
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map_or(0, |i| i + 1);
        let line = &line[..end];
        if line.is_empty() {
            return None;
        }
        let invalid = || Some(error(Value::Null, -32600, "invalid request", None));
        let Ok(msg) = strict_json::from_slice(line) else {
            return Some(error(Value::Null, -32700, "parse error", None));
        };
        let Some(obj) = msg.as_object() else {
            return invalid();
        };
        if obj.get("jsonrpc") != Some(&json!("2.0")) {
            return invalid();
        }
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            return invalid();
        };
        let id = match obj.get("id") {
            None => return None,
            Some(id) if id.is_string() || id.is_number() => id.clone(),
            Some(_) => return invalid(),
        };
        match method {
            "initialize" => Some(ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "reach-cu-mcp", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Read-only observation through cu-agent. No result authorizes input. A result with needs_agent=true must be reviewed by the agent; do not retry it automatically.",
                }),
            )),
            "ping" => Some(ok(id, json!({}))),
            "tools/list" => {
                let request =
                    serde_json::to_vec(&json!({"id": "mcp-describe", "method": "describe"}))
                        .unwrap_or_default();
                let s = outcome((self.run)(&request));
                if s["status"] == "ok" {
                    if let Ok(tools) = tools_from_describe(&s["reply"]) {
                        return Some(ok(id, json!({"tools": tools})));
                    }
                }
                Some(error(id, -32603, "cu-agent describe failed", Some(s)))
            }
            "tools/call" => {
                let Some(params) = obj.get("params").and_then(Value::as_object) else {
                    return Some(invalid_params(id));
                };
                let name = match params.get("name").and_then(Value::as_str) {
                    Some(name) if !name.is_empty() => name,
                    _ => return Some(invalid_params(id)),
                };
                let mut request = match params.get("arguments") {
                    None => Map::new(),
                    Some(Value::Object(arguments)) => arguments.clone(),
                    Some(_) => return Some(invalid_params(id)),
                };
                if request.contains_key("id") || request.contains_key("method") {
                    return Some(invalid_params(id));
                }
                self.calls += 1;
                request.insert("id".into(), json!(format!("mcp-{}", self.calls)));
                request.insert("method".into(), json!(name));
                let bytes = serde_json::to_vec(&Value::Object(request)).unwrap_or_default();
                let s = outcome((self.run)(&bytes));
                let is_error = s["status"] != "ok" && s["status"] != "unknown";
                Some(ok(
                    id,
                    json!({
                        "content": [{"type": "text", "text": s.to_string()}],
                        "structuredContent": s,
                        "isError": is_error,
                    }),
                ))
            }
            _ => Some(error(id, -32601, "method not found", None)),
        }
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut err = json!({"code": code, "message": message});
    if let Some(data) = data {
        err["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": err})
}

fn invalid_params(id: Value) -> Value {
    error(id, -32602, "invalid params", None)
}

const INVALID_REPLY: &str = "cu-agent reply is not a valid response";

fn parse_reply(out: Result<AgentOutput, String>) -> Result<(Value, Option<i32>), String> {
    let out = out?;
    let reply = strict_json::from_slice(&out.stdout).map_err(|_| INVALID_REPLY.to_string())?;
    let valid =
        reply.is_object() && reply["action_allowed"] == false && reply["status"].is_string();
    if !valid {
        return Err(INVALID_REPLY.into());
    }
    Ok((reply, out.exit_code))
}

fn structured(status: &str, exit_code: Option<i32>, reply: Value, reason: Option<String>) -> Value {
    let mut s = json!({
        "action_allowed": false,
        "status": status,
        "needs_agent": status != "ok",
        "exit_code": exit_code,
        "reply": reply,
    });
    if let Some(reason) = reason {
        s["reason"] = reason.into();
    }
    s
}

fn outcome(out: Result<AgentOutput, String>) -> Value {
    match parse_reply(out) {
        Ok((reply, exit_code)) => {
            let status = reply["status"].as_str().unwrap_or_default().to_string();
            structured(&status, exit_code, reply, None)
        }
        Err(reason) => structured("wrapper_error", None, Value::Null, Some(reason)),
    }
}

fn string_list(value: &Value) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or("describe field list is not an array")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| "describe field is not a string".to_string())
        })
        .collect()
}

pub fn tools_from_describe(reply: &Value) -> Result<Vec<Value>, String> {
    let methods = reply["result"]["methods"]
        .as_object()
        .ok_or("describe reply has no methods object")?;
    let mut tools = Vec::new();
    for (name, entry) in methods {
        let fields = string_list(&entry["fields"])?;
        let agent_present = entry.get("agent_fields").is_some();
        let agent = match entry.get("agent_fields") {
            Some(v) => string_list(v)?,
            None => Vec::new(),
        };
        let agent_optional = match entry.get("agent_optional_fields") {
            Some(v) => string_list(v)?,
            None => Vec::new(),
        };
        let mut properties = Map::new();
        for field in fields.iter().chain(&agent).chain(&agent_optional) {
            properties.insert(field.clone(), json!({}));
        }
        let required: Vec<String> = if agent_present {
            fields
                .iter()
                .filter(|f| agent.contains(f))
                .cloned()
                .collect()
        } else {
            fields.clone()
        };
        let availability = entry["availability"].as_str().unwrap_or("read-only");
        tools.push(json!({
            "name": name,
            "description": format!("cu-agent method {name}: {availability}. cu-agent validates every field. No result authorizes input."),
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            },
        }));
    }
    Ok(tools)
}

pub fn serve<I, O, R>(mut input: I, mut output: O, run: R) -> io::Result<()>
where
    I: BufRead,
    O: Write,
    R: FnMut(&[u8]) -> Result<AgentOutput, String>,
{
    let mut server = Server::new(run);
    loop {
        let mut line = Vec::new();
        let n = (&mut input)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        let reply = if line.last() != Some(&b'\n') && line.len() > MAX_LINE {
            // 丟掉這一行剩下的部分。
            loop {
                let mut rest = Vec::new();
                let m = (&mut input)
                    .take(MAX_LINE as u64)
                    .read_until(b'\n', &mut rest)?;
                if m == 0 || rest.last() == Some(&b'\n') {
                    break;
                }
            }
            Some(error(Value::Null, -32600, "request too large", None))
        } else {
            server.handle_line(&line)
        };
        if let Some(reply) = reply {
            output.write_all(reply.to_string().as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

pub fn run_cu_agent(
    program: &Path,
    request: &[u8],
    timeout: Duration,
) -> Result<AgentOutput, String> {
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start cu-agent: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let request = request.to_vec();
    // 寫入失敗（例如對方已關閉）不是錯誤：以回覆為準。
    thread::spawn(move || {
        let _ = stdin.write_all(&request);
    });
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout.take(MAX_REPLY + 1).read_to_end(&mut bytes);
        let _ = tx.send(read.map(|_| bytes));
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("cu-agent timed out; its helpers may remain".into());
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cu-agent wait failed: {e}"));
            }
        }
    };
    let bytes = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "cu-agent output did not close before the deadline".to_string())?
        .map_err(|e| format!("cannot read cu-agent output: {e}"))?;
    if bytes.len() as u64 > MAX_REPLY {
        return Err("cu-agent reply too large".into());
    }
    Ok(AgentOutput {
        stdout: bytes,
        exit_code: status.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::{Cell, RefCell};

    fn reply_with(status: &str, code: i32, result: Value) -> Result<AgentOutput, String> {
        let body = json!({
            "protocol": "mac-computer-use-agent-v1",
            "id": "x",
            "method": "m",
            "status": status,
            "action_allowed": false,
            "result": result,
        });
        Ok(AgentOutput {
            stdout: body.to_string().into_bytes(),
            exit_code: Some(code),
        })
    }

    fn reply(status: &str, code: i32) -> Result<AgentOutput, String> {
        reply_with(status, code, json!({}))
    }

    fn parse(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn initialize_reports_version_and_tools_only() {
        let calls = Cell::new(0);
        let mut server = Server::new(|_: &[u8]| {
            calls.set(calls.get() + 1);
            reply("ok", 0)
        });
        let v = server
            .handle_line(
                br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
            )
            .unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(v["result"]["capabilities"], json!({"tools": {}}));
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn notifications_and_blank_lines_get_no_reply() {
        let calls = Cell::new(0);
        let mut server = Server::new(|_: &[u8]| {
            calls.set(calls.get() + 1);
            reply("ok", 0)
        });
        assert!(server
            .handle_line(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        assert!(server.handle_line(b"\n").is_none());
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn tools_list_is_built_from_describe() {
        let seen = RefCell::new(Vec::<Vec<u8>>::new());
        let methods = json!({
            "action": {"fields": ["event"], "availability": "unsupported"},
            "capture": {
                "fields": ["pid", "window_id", "image", "capture_helper", "ax_helper"],
                "agent_fields": ["pid", "window_id", "image"]
            },
            "describe": {"fields": []}
        });
        let mut server = Server::new(|request: &[u8]| {
            seen.borrow_mut().push(request.to_vec());
            reply_with("ok", 0, json!({"methods": methods.clone()}))
        });
        let v = server
            .handle_line(br#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#)
            .unwrap();
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(
            parse(&seen.borrow()[0]),
            json!({"id": "mcp-describe", "method": "describe"})
        );
        assert_eq!(v["id"], 3);
        let tools = v["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["action", "capture", "describe"]);
        let capture = &tools[1];
        assert_eq!(
            capture["inputSchema"]["required"],
            json!(["pid", "window_id", "image"])
        );
        assert_eq!(
            capture["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .len(),
            5
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        }
    }

    #[test]
    fn tools_list_fails_without_a_fallback() {
        let calls = Cell::new(0);
        let mut server = Server::new(|_: &[u8]| {
            calls.set(calls.get() + 1);
            reply("error", 1)
        });
        let v = server
            .handle_line(br#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#)
            .unwrap();
        assert_eq!(v["error"]["code"], -32603);
        assert_eq!(v["error"]["data"]["action_allowed"], false);
        assert_eq!(v["error"]["data"]["status"], "error");
        assert!(v.get("result").is_none());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn tools_call_forwards_exactly_one_request() {
        let seen = RefCell::new(Vec::<Vec<u8>>::new());
        let mut server = Server::new(|request: &[u8]| {
            seen.borrow_mut().push(request.to_vec());
            reply("unknown", 3)
        });
        let v = server
            .handle_line(
                br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"windows","arguments":{"pid":7}}}"#,
            )
            .unwrap();
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(
            parse(&seen.borrow()[0]),
            json!({"id": "mcp-1", "method": "windows", "pid": 7})
        );
        let result = &v["result"];
        assert_eq!(result["isError"], false);
        let s = &result["structuredContent"];
        assert_eq!(s["status"], "unknown");
        assert_eq!(s["needs_agent"], true);
        assert_eq!(s["exit_code"], 3);
        assert_eq!(s["action_allowed"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(parse(text.as_bytes()), *s);
    }

    #[test]
    fn ok_reply_needs_no_agent() {
        let mut server = Server::new(|_: &[u8]| reply("ok", 0));
        let v = server
            .handle_line(
                br#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"windows","arguments":{"pid":7}}}"#,
            )
            .unwrap();
        assert_eq!(v["result"]["isError"], false);
        assert_eq!(v["result"]["structuredContent"]["needs_agent"], false);
    }

    #[test]
    fn error_incomplete_and_unsupported_are_reported_as_is() {
        let cases = [
            ("error", 1),
            ("observation_incomplete", 1),
            ("unsupported", 3),
            ("agent_unavailable", 3),
        ];
        for (status, code) in cases {
            let calls = Cell::new(0);
            let mut server = Server::new(|_: &[u8]| {
                calls.set(calls.get() + 1);
                reply(status, code)
            });
            let v = server
                .handle_line(
                    br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"windows","arguments":{"pid":7}}}"#,
                )
                .unwrap();
            let result = &v["result"];
            assert_eq!(result["isError"], true, "{status}");
            let s = &result["structuredContent"];
            assert_eq!(s["needs_agent"], true, "{status}");
            assert_eq!(s["status"], status, "{status}");
            assert_eq!(s["reply"]["status"], status, "{status}");
            assert_eq!(calls.get(), 1, "{status}");
        }
    }

    #[test]
    fn reply_that_allows_action_is_refused() {
        let mut server = Server::new(|_: &[u8]| {
            Ok(AgentOutput {
                stdout: br#"{"status":"ok","action_allowed":true}"#.to_vec(),
                exit_code: Some(0),
            })
        });
        let v = server
            .handle_line(
                br#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"windows","arguments":{"pid":7}}}"#,
            )
            .unwrap();
        let result = &v["result"];
        let s = &result["structuredContent"];
        assert_eq!(s["status"], "wrapper_error");
        assert_eq!(s["action_allowed"], false);
        assert!(s["reply"].is_null());
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn runner_failure_and_garbage_are_wrapper_errors() {
        let mut failing = Server::new(|_: &[u8]| Err("cannot start cu-agent: x".to_string()));
        let v = failing
            .handle_line(
                br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"windows"}}"#,
            )
            .unwrap();
        let s = &v["result"]["structuredContent"];
        assert_eq!(s["status"], "wrapper_error");
        assert_eq!(s["action_allowed"], false);
        assert_eq!(v["result"]["isError"], true);
        assert!(s["reason"].as_str().unwrap().contains("cannot start"));

        let mut garbage = Server::new(|_: &[u8]| {
            Ok(AgentOutput {
                stdout: b"not json".to_vec(),
                exit_code: Some(0),
            })
        });
        let v = garbage
            .handle_line(
                br#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"windows"}}"#,
            )
            .unwrap();
        let s = &v["result"]["structuredContent"];
        assert_eq!(s["status"], "wrapper_error");
        assert_eq!(s["action_allowed"], false);
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn invalid_params_never_reach_cu_agent() {
        let lines = [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":"x"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":5}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":""}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"windows","arguments":[]}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"windows","arguments":{"id":"z"}}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"windows","arguments":{"method":"z"}}}"#,
        ];
        let calls = Cell::new(0);
        let mut server = Server::new(|_: &[u8]| {
            calls.set(calls.get() + 1);
            reply("ok", 0)
        });
        for line in lines {
            let v = server.handle_line(line.as_bytes()).unwrap();
            assert_eq!(v["error"]["code"], -32602, "{line}");
        }
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn protocol_errors_have_fixed_codes() {
        let calls = Cell::new(0);
        let mut server = Server::new(|_: &[u8]| {
            calls.set(calls.get() + 1);
            reply("ok", 0)
        });
        let cases = [
            ("not json", -32700),
            (r#"{"jsonrpc":"2.0","id":1,"id":2,"method":"ping"}"#, -32700),
            ("[]", -32600),
            (r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#, -32600),
            (
                r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#,
                -32601,
            ),
        ];
        for (line, code) in cases {
            let v = server.handle_line(line.as_bytes()).unwrap();
            assert_eq!(v["error"]["code"], code, "{line}");
        }
        let v = server
            .handle_line(br#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#)
            .unwrap();
        assert_eq!(v["result"], json!({}));
        assert_eq!(v["id"], "p");
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn serve_refuses_an_oversized_line_and_continues() {
        let mut input = vec![b'a'; 65_537];
        input.push(b'\n');
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
        input.push(b'\n');
        let mut out = Vec::new();
        serve(&input[..], &mut out, |_: &[u8]| reply("ok", 0)).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32600);
        assert_eq!(lines[0]["error"]["message"], "request too large");
        assert!(lines[0]["id"].is_null());
        assert_eq!(lines[1]["id"], 2);
        assert!(lines[1].get("result").is_some());
    }

    #[test]
    fn serve_accepts_a_line_of_exactly_the_limit() {
        let mut line = br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#.to_vec();
        line.resize(MAX_LINE, b' ');
        line.push(b'\n');
        let mut out = Vec::new();
        serve(&line[..], &mut out, |_: &[u8]| reply("ok", 0)).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].get("result").is_some());
    }
}
