//! MCP stdio server. Every tool call runs one `cu-agent` process and returns its reply.
//! It never implements capture, accessibility, or input.
//! Executable paths are fixed when the server starts; a tool argument never names one.
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
/// Methods that can send input. MCP never offers them in `tools/list` and refuses them in
/// `tools/call` before any `cu-agent` process runs, whatever the ReachAgent arming state.
/// Input is a local `cu-agent` request only (plan I-2, faberline/reach#27).
pub const INPUT_METHODS: &[&str] = &["action"];
/// The tool error for a refused input method.
pub const INPUT_NOT_EXPOSED: &str =
    "input methods are not exposed through MCP; MCP cannot send or enable input";

pub struct AgentOutput {
    pub stdout: Vec<u8>,
    pub exit_code: Option<i32>,
}

pub struct Server<R: FnMut(&[u8]) -> Result<AgentOutput, String>> {
    run: R,
    calls: u64,
    helpers: Map<String, Value>,
    methods: Option<Map<String, Value>>,
}

impl<R: FnMut(&[u8]) -> Result<AgentOutput, String>> Server<R> {
    /// `helpers` 是啟動時固定的執行檔路徑，見 [`parse_helpers`]。
    pub fn new(run: R, helpers: Map<String, Value>) -> Self {
        Self {
            run,
            calls: 0,
            helpers,
            methods: None,
        }
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
                    if let Ok(tools) = tools_from_describe(&s["reply"], &self.helpers) {
                        self.methods = s["reply"]["result"]["methods"].as_object().cloned();
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
                // Before any argument check: the refusal must not depend on the arguments.
                if INPUT_METHODS.contains(&name) {
                    let s =
                        structured("refused", None, Value::Null, Some(INPUT_NOT_EXPOSED.into()));
                    return Some(tool_result(id, s));
                }
                let mut request = match params.get("arguments") {
                    None => Map::new(),
                    Some(Value::Object(arguments)) => arguments.clone(),
                    Some(_) => return Some(invalid_params(id)),
                };
                if request.contains_key("id") || request.contains_key("method") {
                    return Some(invalid_params(id));
                }
                if let Some(field) = request.keys().find(|k| is_executable_field(k)) {
                    let data = format!(
                        "{field} is fixed when cu-mcp starts; a tool argument cannot name an executable"
                    );
                    return Some(error(id, -32602, "invalid params", Some(json!(data))));
                }
                match self.fixed_fields(name) {
                    Ok(fixed) => request.extend(fixed),
                    Err(reason) => {
                        let s = structured("wrapper_error", None, Value::Null, Some(reason));
                        return Some(tool_result(id, s));
                    }
                }
                self.calls += 1;
                request.insert("id".into(), json!(format!("mcp-{}", self.calls)));
                request.insert("method".into(), json!(name));
                let bytes = serde_json::to_vec(&Value::Object(request)).unwrap_or_default();
                Some(tool_result(id, outcome((self.run)(&bytes))))
            }
            _ => Some(error(id, -32601, "method not found", None)),
        }
    }

    /// `name` 這個方法要加上的固定路徑。方法表只在第一次需要時向 `cu-agent` 讀一次。
    /// 有 `agent_fields` 的方法只走 ReachAgent，不加 helper。方法表沒有的名稱不加任何欄位。
    fn fixed_fields(&mut self, name: &str) -> Result<Map<String, Value>, String> {
        if self.methods.is_none() {
            let request = serde_json::to_vec(&json!({"id": "mcp-describe", "method": "describe"}))
                .unwrap_or_default();
            let (reply, _) = parse_reply((self.run)(&request))
                .map_err(|reason| format!("cu-agent describe failed: {reason}"))?;
            let methods = reply["result"]["methods"].as_object();
            match methods {
                Some(methods) if reply["status"] == "ok" => self.methods = Some(methods.clone()),
                _ => return Err("cu-agent describe failed; the call was not made".into()),
            }
        }
        let mut fixed = Map::new();
        let Some(entry) = self.methods.as_ref().and_then(|m| m.get(name)) else {
            return Ok(fixed);
        };
        if entry.get("agent_fields").is_some() {
            return Ok(fixed);
        }
        for field in string_list(&entry["fields"])? {
            if !is_executable_field(&field) {
                continue;
            }
            let Some(path) = self.helpers.get(&field) else {
                return Err(format!(
                    "{field} was not fixed when cu-mcp started; add --helper {field}=<absolute path>"
                ));
            };
            fixed.insert(field, path.clone());
        }
        Ok(fixed)
    }
}

fn tool_result(id: Value, s: Value) -> Value {
    let is_error = s["status"] != "ok" && s["status"] != "unknown";
    ok(
        id,
        json!({
            "content": [{"type": "text", "text": s.to_string()}],
            "structuredContent": s,
            "isError": is_error,
        }),
    )
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
    let valid = reply.is_object()
        && reply["action_allowed"] == false
        && reply["status"].is_string()
        && !allows_action(&reply);
    if !valid {
        return Err(INVALID_REPLY.into());
    }
    Ok((reply, out.exit_code))
}

/// 任何層級只要有不是 `false` 的 `action_allowed` 就回傳 `true`。
fn allows_action(reply: &Value) -> bool {
    let mut pending = vec![reply];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "action_allowed" && child != &Value::Bool(false) {
                        return true;
                    }
                    pending.push(child);
                }
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
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

/// 會被 `cu-agent` 當成執行檔路徑的欄位：`*_helper`、`adapter`、`adapters`。
pub fn is_executable_field(name: &str) -> bool {
    name.ends_with("_helper") || name == "adapter" || name == "adapters"
}

const MAX_ADAPTERS: usize = 4;

/// 解析 `--helper <欄位>=<絕對路徑>`。`adapters` 可重複 1 到 4 次，其他欄位只能出現一次。
pub fn parse_helpers(args: &[String]) -> Result<Map<String, Value>, String> {
    let mut helpers = Map::new();
    for pair in args.chunks(2) {
        let [flag, value] = pair else {
            return Err("--helper needs a value".into());
        };
        if flag != "--helper" {
            return Err(format!("unknown argument {flag}"));
        }
        let (field, path) = value
            .split_once('=')
            .ok_or("--helper needs <field>=<absolute path>")?;
        if !is_executable_field(field) || field == "_helper" {
            return Err(format!("{field} is not a helper field"));
        }
        if !Path::new(path).is_absolute() {
            return Err(format!("{field} needs an absolute path"));
        }
        if field == "adapters" {
            let list = helpers.entry(field).or_insert_with(|| json!([]));
            let Some(list) = list.as_array_mut() else {
                return Err("adapters is not a list".into());
            };
            if list.len() == MAX_ADAPTERS || list.contains(&json!(path)) {
                return Err("adapters needs 1 to 4 distinct paths".into());
            }
            list.push(json!(path));
        } else if helpers.insert(field.into(), json!(path)).is_some() {
            return Err(format!("{field} was given twice"));
        }
    }
    Ok(helpers)
}

/// 工具清單。執行檔路徑欄位不列為參數；缺少固定路徑的方法不提供，
/// 除非它有 `agent_fields`（那些方法只走 ReachAgent）。
pub fn tools_from_describe(
    reply: &Value,
    helpers: &Map<String, Value>,
) -> Result<Vec<Value>, String> {
    let methods = reply["result"]["methods"]
        .as_object()
        .ok_or("describe reply has no methods object")?;
    let mut tools = Vec::new();
    for (name, entry) in methods {
        if INPUT_METHODS.contains(&name.as_str()) {
            continue;
        }
        let all_fields = string_list(&entry["fields"])?;
        let agent_present = entry.get("agent_fields").is_some();
        let agent = match entry.get("agent_fields") {
            Some(v) => string_list(v)?,
            None => Vec::new(),
        };
        let agent_optional = match entry.get("agent_optional_fields") {
            Some(v) => string_list(v)?,
            None => Vec::new(),
        };
        let (fixed, fields): (Vec<String>, Vec<String>) =
            all_fields.into_iter().partition(|f| is_executable_field(f));
        if !agent_present && fixed.iter().any(|f| !helpers.contains_key(f)) {
            continue;
        }
        let mut properties = Map::new();
        for field in fields.iter().chain(&agent).chain(&agent_optional) {
            if !is_executable_field(field) {
                properties.insert(field.clone(), json!({}));
            }
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

pub fn serve<I, O, R>(
    mut input: I,
    mut output: O,
    run: R,
    helpers: Map<String, Value>,
) -> io::Result<()>
where
    I: BufRead,
    O: Write,
    R: FnMut(&[u8]) -> Result<AgentOutput, String>,
{
    let mut server = Server::new(run, helpers);
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

    const CALL_WINDOWS: &[u8] = br#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"windows","arguments":{"pid":7}}}"#;

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

    fn methods() -> Value {
        json!({
            "action": {"fields": ["event"], "availability": "unsupported"},
            "capture": {
                "fields": ["pid", "window_id", "image", "capture_helper", "ax_helper"],
                "agent_fields": ["pid", "window_id", "image"]
            },
            "describe": {"fields": []},
            "find_bundle": {"fields": ["bundle_id", "ax_helper"]},
            "interpret_image": {"fields": ["image", "adapters", "output"]},
            "recognize_text": {"fields": ["image", "ocr_helper"]},
            "windows": {"fields": ["pid"]}
        })
    }

    fn helpers(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// 假的 cu-agent：`describe` 回方法表；其他請求記到 `seen`，再回 `answer`。
    fn fake<'a>(
        describes: &'a Cell<u32>,
        seen: &'a RefCell<Vec<Value>>,
        answer: impl Fn() -> Result<AgentOutput, String> + 'a,
    ) -> impl FnMut(&[u8]) -> Result<AgentOutput, String> + 'a {
        move |request: &[u8]| {
            let v = parse(request);
            if v["method"] == "describe" && v["id"] == "mcp-describe" {
                describes.set(describes.get() + 1);
                return reply_with("ok", 0, json!({"methods": methods()}));
            }
            seen.borrow_mut().push(v);
            answer()
        }
    }

    fn call(name: &str, arguments: Value) -> Vec<u8> {
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
            .to_string()
            .into_bytes()
    }

    #[test]
    fn initialize_reports_version_and_tools_only() {
        let calls = Cell::new(0);
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("ok", 0)
            },
            Map::new(),
        );
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
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("ok", 0)
            },
            Map::new(),
        );
        assert!(server
            .handle_line(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        assert!(server.handle_line(b"\n").is_none());
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn tools_list_is_built_from_describe() {
        let seen = RefCell::new(Vec::<Vec<u8>>::new());
        let mut server = Server::new(
            |request: &[u8]| {
                seen.borrow_mut().push(request.to_vec());
                reply_with("ok", 0, json!({"methods": methods()}))
            },
            helpers(&[("ax_helper", json!("/opt/reach/ax"))]),
        );
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
        // `interpret_image` 與 `recognize_text` 的執行檔沒有在啟動時指定，所以不提供。
        assert_eq!(names, ["capture", "describe", "find_bundle", "windows"]);
        let capture = &tools[0];
        assert_eq!(
            capture["inputSchema"]["required"],
            json!(["pid", "window_id", "image"])
        );
        assert_eq!(
            capture["inputSchema"]["properties"],
            json!({"pid": {}, "window_id": {}, "image": {}})
        );
        let find_bundle = &tools[2];
        assert_eq!(find_bundle["inputSchema"]["required"], json!(["bundle_id"]));
        assert_eq!(
            find_bundle["inputSchema"]["properties"],
            json!({"bundle_id": {}})
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            for key in tool["inputSchema"]["properties"]
                .as_object()
                .unwrap()
                .keys()
            {
                assert!(!is_executable_field(key), "{key}");
            }
        }
    }

    /// A describe reply whose `action` entry looks fully usable and whose agent is armed.
    fn armed_describe() -> Value {
        let mut m = methods();
        m["action"] = json!({
            "fields": ["delivery", "target", "observation", "event", "evidence_dir"],
            "agent_fields": ["delivery", "target", "observation", "event", "evidence_dir"],
            "availability": "foreground input through ReachAgent"
        });
        json!({"methods": m, "agent": {"state": "running",
            "input": {"state": "armed", "events_remaining": 20}}})
    }

    #[test]
    fn tools_list_never_offers_input_methods_whatever_the_arming() {
        assert_eq!(INPUT_METHODS, ["action"]);
        for (describe, helpers) in [
            (json!({"methods": methods()}), Map::new()),
            (armed_describe(), Map::new()),
            (
                armed_describe(),
                helpers(&[("ax_helper", json!("/opt/reach/ax"))]),
            ),
        ] {
            let tools = tools_from_describe(&json!({"result": describe}), &helpers).unwrap();
            let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
            for input in INPUT_METHODS {
                assert!(!names.contains(input), "{input} offered in {names:?}");
            }
            assert!(names.contains(&"describe"));
        }
    }

    #[test]
    fn tools_call_refuses_input_methods_before_running_cu_agent() {
        let calls = Cell::new(0);
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply_with("ok", 0, armed_describe())
            },
            Map::new(),
        );
        // Load the armed method table first, so the refusal cannot come from a missing table.
        server
            .handle_line(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap();
        assert_eq!(calls.get(), 1);
        let foreground = json!({"delivery": "foreground",
            "target": {"bundle_id": "com.apple.Safari", "pid": 1, "window_id": 1,
                       "process_start_unix_seconds": 1.0},
            "observation": {"image_sha256": "a".repeat(64)},
            "event": {"kind": "click", "button": "left", "x": 1, "y": 1},
            "evidence_dir": "/tmp/e"});
        for arguments in [
            foreground,
            json!({"event": {"kind": "click"}}),
            json!({}),
            json!({"capture_helper": "/x"}),
            json!({"id": "x"}),
            json!("not an object"),
        ] {
            let v = server.handle_line(&call("action", arguments)).unwrap();
            let s = &v["result"]["structuredContent"];
            assert_eq!(v["result"]["isError"], true);
            assert_eq!(s["status"], "refused");
            assert_eq!(s["action_allowed"], false);
            assert_eq!(s["reason"], INPUT_NOT_EXPOSED);
            assert!(s["reply"].is_null());
        }
        assert_eq!(
            calls.get(),
            1,
            "no cu-agent process ran for an input method"
        );
    }

    #[test]
    fn tools_list_fails_without_a_fallback() {
        let calls = Cell::new(0);
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("error", 1)
            },
            Map::new(),
        );
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
        let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
        let mut server = Server::new(fake(&describes, &seen, || reply("unknown", 3)), Map::new());
        let v = server.handle_line(CALL_WINDOWS).unwrap();
        assert_eq!(
            *seen.borrow(),
            [json!({"id": "mcp-1", "method": "windows", "pid": 7})]
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

        // 方法表只讀一次；之後每次呼叫只啟動一個 cu-agent。
        server.handle_line(CALL_WINDOWS).unwrap();
        assert_eq!(seen.borrow().len(), 2);
        assert_eq!(seen.borrow()[1]["id"], "mcp-2");
        assert_eq!(describes.get(), 1);
    }

    #[test]
    fn tools_call_adds_the_paths_fixed_at_startup() {
        let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
        let fixed = helpers(&[
            ("ax_helper", json!("/opt/reach/ax")),
            ("capture_helper", json!("/opt/reach/capture")),
            ("adapters", json!(["/opt/reach/a", "/opt/reach/b"])),
        ]);
        let mut server = Server::new(fake(&describes, &seen, || reply("ok", 0)), fixed);
        server
            .handle_line(&call(
                "find_bundle",
                json!({"bundle_id": "com.example.app"}),
            ))
            .unwrap();
        server
            .handle_line(&call(
                "interpret_image",
                json!({"image": "/tmp/a.png", "output": "/tmp/a.json"}),
            ))
            .unwrap();
        // 有 `agent_fields` 的方法只走 ReachAgent，不加 helper。
        server
            .handle_line(&call(
                "capture",
                json!({"pid": 7, "window_id": 9, "image": "/tmp/c.png"}),
            ))
            .unwrap();
        // 方法表沒有的名稱照樣轉送，由 cu-agent 回報。
        server
            .handle_line(&call("no_such_method", json!({})))
            .unwrap();
        assert_eq!(
            *seen.borrow(),
            [
                json!({"id": "mcp-1", "method": "find_bundle", "bundle_id": "com.example.app", "ax_helper": "/opt/reach/ax"}),
                json!({"id": "mcp-2", "method": "interpret_image", "image": "/tmp/a.png", "output": "/tmp/a.json", "adapters": ["/opt/reach/a", "/opt/reach/b"]}),
                json!({"id": "mcp-3", "method": "capture", "pid": 7, "window_id": 9, "image": "/tmp/c.png"}),
                json!({"id": "mcp-4", "method": "no_such_method"}),
            ]
        );
    }

    #[test]
    fn executable_path_arguments_never_reach_cu_agent() {
        let fields = [
            "ax_helper",
            "capture_helper",
            "ocr_helper",
            "adapter",
            "adapters",
            "overlay_helper",
            "stream_helper",
            "notification_helper",
            "session_helper",
            "isolation_helper",
            "future_helper",
        ];
        let calls = Cell::new(0);
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("ok", 0)
            },
            helpers(&[("ax_helper", json!("/opt/reach/ax"))]),
        );
        for field in fields {
            for name in ["find_bundle", "windows", "no_such_method"] {
                let mut arguments = json!({"bundle_id": "com.example.app"});
                arguments[field] = json!("/bin/sh");
                let v = server.handle_line(&call(name, arguments)).unwrap();
                assert_eq!(v["error"]["code"], -32602, "{name} {field}");
                assert!(
                    v["error"]["data"].as_str().unwrap().contains(field),
                    "{name} {field}"
                );
            }
        }
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn method_without_its_fixed_helper_is_not_run() {
        let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
        let mut server = Server::new(
            fake(&describes, &seen, || reply("ok", 0)),
            helpers(&[("ax_helper", json!("/opt/reach/ax"))]),
        );
        let v = server
            .handle_line(&call("recognize_text", json!({"image": "/tmp/a.png"})))
            .unwrap();
        let result = &v["result"];
        assert_eq!(result["isError"], true);
        let s = &result["structuredContent"];
        assert_eq!(s["status"], "wrapper_error");
        assert_eq!(s["action_allowed"], false);
        assert_eq!(s["needs_agent"], true);
        assert!(s["reply"].is_null());
        assert!(s["reason"].as_str().unwrap().contains("ocr_helper"));
        assert!(seen.borrow().is_empty());
    }

    #[test]
    fn tools_call_without_a_method_table_is_not_run() {
        let calls = Cell::new(0);
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("error", 1)
            },
            Map::new(),
        );
        let v = server.handle_line(CALL_WINDOWS).unwrap();
        let s = &v["result"]["structuredContent"];
        assert_eq!(s["status"], "wrapper_error");
        assert_eq!(v["result"]["isError"], true);
        assert!(s["reason"].as_str().unwrap().contains("describe"));
        // 只有那一次 describe；請求本身沒有送出。
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn ok_reply_needs_no_agent() {
        let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
        let mut server = Server::new(fake(&describes, &seen, || reply("ok", 0)), Map::new());
        let v = server.handle_line(CALL_WINDOWS).unwrap();
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
            let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
            let mut server =
                Server::new(fake(&describes, &seen, || reply(status, code)), Map::new());
            let v = server.handle_line(CALL_WINDOWS).unwrap();
            let result = &v["result"];
            assert_eq!(result["isError"], true, "{status}");
            let s = &result["structuredContent"];
            assert_eq!(s["needs_agent"], true, "{status}");
            assert_eq!(s["status"], status, "{status}");
            assert_eq!(s["reply"]["status"], status, "{status}");
            assert_eq!(seen.borrow().len(), 1, "{status}");
        }
    }

    #[test]
    fn reply_that_allows_action_is_refused() {
        let bodies = [
            r#"{"status":"ok","action_allowed":true}"#,
            r#"{"status":"ok"}"#,
            r#"{"status":"ok","action_allowed":false,"result":{"action_allowed":true}}"#,
            r#"{"status":"ok","action_allowed":false,"result":{"items":[{"deep":[[{"action_allowed":true}]]}]}}"#,
            r#"{"status":"ok","action_allowed":false,"result":{"action_allowed":null}}"#,
            r#"{"status":"ok","action_allowed":false,"result":{"action_allowed":"false"}}"#,
            r#"{"status":"ok","action_allowed":false,"result":{"action_allowed":0}}"#,
            r#"{"status":"ok","action_allowed":false,"result":{"action_allowed":{}}}"#,
        ];
        for body in bodies {
            let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
            let mut server = Server::new(
                fake(&describes, &seen, || {
                    Ok(AgentOutput {
                        stdout: body.as_bytes().to_vec(),
                        exit_code: Some(0),
                    })
                }),
                Map::new(),
            );
            let v = server.handle_line(CALL_WINDOWS).unwrap();
            assert_eq!(seen.borrow().len(), 1, "{body}");
            let result = &v["result"];
            let s = &result["structuredContent"];
            assert_eq!(s["status"], "wrapper_error", "{body}");
            assert_eq!(s["action_allowed"], false, "{body}");
            assert!(s["reply"].is_null(), "{body}");
            assert_eq!(result["isError"], true, "{body}");
            assert!(!v.to_string().contains("\"action_allowed\":true"), "{body}");
        }
    }

    #[test]
    fn nested_false_action_allowed_is_accepted() {
        let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
        let mut server = Server::new(
            fake(&describes, &seen, || {
                reply_with("ok", 0, json!({"items": [{"action_allowed": false}]}))
            }),
            Map::new(),
        );
        let v = server.handle_line(CALL_WINDOWS).unwrap();
        assert_eq!(v["result"]["structuredContent"]["status"], "ok");
        assert_eq!(v["result"]["isError"], false);
    }

    #[test]
    fn describe_that_allows_action_gives_no_tools() {
        let mut server = Server::new(
            |_: &[u8]| {
                reply_with(
                    "ok",
                    0,
                    json!({"methods": methods(), "agent": {"action_allowed": true}}),
                )
            },
            Map::new(),
        );
        let v = server
            .handle_line(br#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#)
            .unwrap();
        assert_eq!(v["error"]["code"], -32603);
        assert_eq!(v["error"]["data"]["status"], "wrapper_error");
    }

    #[test]
    fn runner_failure_and_garbage_are_wrapper_errors() {
        let (describes, seen) = (Cell::new(0), RefCell::new(Vec::new()));
        let mut failing = Server::new(
            fake(&describes, &seen, || {
                Err("cannot start cu-agent: x".to_string())
            }),
            Map::new(),
        );
        let v = failing.handle_line(CALL_WINDOWS).unwrap();
        let s = &v["result"]["structuredContent"];
        assert_eq!(s["status"], "wrapper_error");
        assert_eq!(s["action_allowed"], false);
        assert_eq!(v["result"]["isError"], true);
        assert!(s["reason"].as_str().unwrap().contains("cannot start"));

        let mut garbage = Server::new(
            fake(&describes, &seen, || {
                Ok(AgentOutput {
                    stdout: b"not json".to_vec(),
                    exit_code: Some(0),
                })
            }),
            Map::new(),
        );
        let v = garbage.handle_line(CALL_WINDOWS).unwrap();
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
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("ok", 0)
            },
            Map::new(),
        );
        for line in lines {
            let v = server.handle_line(line.as_bytes()).unwrap();
            assert_eq!(v["error"]["code"], -32602, "{line}");
        }
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn protocol_errors_have_fixed_codes() {
        let calls = Cell::new(0);
        let mut server = Server::new(
            |_: &[u8]| {
                calls.set(calls.get() + 1);
                reply("ok", 0)
            },
            Map::new(),
        );
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
    fn helper_arguments_fix_absolute_paths() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_helpers(&[]), Ok(Map::new()));
        assert_eq!(
            parse_helpers(&args(&[
                "--helper",
                "ax_helper=/opt/reach/ax",
                "--helper",
                "adapter=/opt/reach/one=1",
                "--helper",
                "adapters=/opt/reach/a",
                "--helper",
                "adapters=/opt/reach/b",
            ])),
            Ok(helpers(&[
                ("ax_helper", json!("/opt/reach/ax")),
                ("adapter", json!("/opt/reach/one=1")),
                ("adapters", json!(["/opt/reach/a", "/opt/reach/b"])),
            ]))
        );
        let refused: [&[&str]; 9] = [
            &["--helper"],
            &["--helper", "ax_helper"],
            &["--helper", "ax_helper="],
            &["--helper", "ax_helper=relative/ax"],
            &["--helper", "image=/tmp/a.png"],
            &["--helper", "=/opt/reach/ax"],
            &["--other", "ax_helper=/opt/reach/ax"],
            &[
                "--helper",
                "ax_helper=/opt/reach/ax",
                "--helper",
                "ax_helper=/opt/reach/other",
            ],
            &[
                "--helper",
                "adapters=/opt/reach/a",
                "--helper",
                "adapters=/opt/reach/a",
            ],
        ];
        for list in refused {
            assert!(parse_helpers(&args(list)).is_err(), "{list:?}");
        }
        let five: Vec<String> = (0..5)
            .flat_map(|i| ["--helper".to_string(), format!("adapters=/opt/reach/{i}")])
            .collect();
        assert!(parse_helpers(&five).is_err());
    }

    #[test]
    fn serve_refuses_an_oversized_line_and_continues() {
        let mut input = vec![b'a'; 65_537];
        input.push(b'\n');
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
        input.push(b'\n');
        let mut out = Vec::new();
        serve(&input[..], &mut out, |_: &[u8]| reply("ok", 0), Map::new()).unwrap();
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
        serve(&line[..], &mut out, |_: &[u8]| reply("ok", 0), Map::new()).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].get("result").is_some());
    }
}
