//! Platform-neutral request and reply types for the display-only cursor overlay.
//!
//! This module describes the overlay that `mac-cursor-overlay` shows on screen.
//! It never sends input and never moves the real cursor. A `visible` overlay is
//! not bound to any window and does not authorize any action.

use serde_json::{json, Map, Value};

pub const PROTOCOL: &str = "cursor-overlay-v1";
pub const SPACE: &str = "global_display_points_top_left";
pub const WINDOW_BINDING: &str = "unbound";
pub const MAX_COORDINATE: f64 = 1_000_000.0;

const INVALID_REPLY: &str = "invalid_reply";

/// A point in global display points, origin at the top left of the primary display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub display_id: u32,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Click,
    Hold,
    Drag,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Click => "click",
            Phase::Hold => "hold",
            Phase::Drag => "drag",
        }
    }

    pub fn parse(text: &str) -> Option<Phase> {
        match text {
            "idle" => Some(Phase::Idle),
            "click" => Some(Phase::Click),
            "hold" => Some(Phase::Hold),
            "drag" => Some(Phase::Drag),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Show(Point),
    Move(Point),
    Phase(Phase),
    Hide,
    Query,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct State {
    pub visible: bool,
    pub phase: Phase,
    pub point: Option<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub id: Option<String>,
    pub reason: Option<String>,
    pub state: State,
}

impl Reply {
    /// Always `false`: the overlay never authorizes an action.
    pub const fn action_allowed(&self) -> bool {
        false
    }
}

/// Parses one request line. Returns the first failing rule as the error reason.
pub fn parse_request(bytes: &[u8]) -> Result<Request, &'static str> {
    let value = crate::strict_json::from_slice(bytes).map_err(|_| "invalid_json")?;
    let map = value.as_object().ok_or("invalid_json")?;

    if map.get("protocol").and_then(Value::as_str) != Some(PROTOCOL) {
        return Err("invalid_request");
    }
    let id = map.get("id").and_then(valid_id).ok_or("invalid_request")?;
    let name = map
        .get("command")
        .and_then(Value::as_str)
        .ok_or("invalid_request")?;

    let keys: &[&str] = match name {
        "show" | "move" => &["protocol", "id", "command", "point"],
        "phase" => &["protocol", "id", "command", "phase"],
        "hide" | "query" | "quit" => &["protocol", "id", "command"],
        _ => return Err("unknown_command"),
    };
    if !has_exact_keys(map, keys) {
        return Err("invalid_request");
    }

    let command = match name {
        "show" => Command::Show(
            map.get("point")
                .and_then(parse_point)
                .ok_or("invalid_point")?,
        ),
        "move" => Command::Move(
            map.get("point")
                .and_then(parse_point)
                .ok_or("invalid_point")?,
        ),
        "phase" => {
            let phase = map
                .get("phase")
                .and_then(Value::as_str)
                .and_then(Phase::parse)
                .ok_or("invalid_phase")?;
            Command::Phase(phase)
        }
        "hide" => Command::Hide,
        "query" => Command::Query,
        _ => Command::Quit,
    };
    Ok(Request { id, command })
}

pub fn request_json(request: &Request) -> Value {
    let id = request.id.as_str();
    match request.command {
        Command::Show(point) => {
            json!({"protocol": PROTOCOL, "id": id, "command": "show", "point": point_json(&point)})
        }
        Command::Move(point) => {
            json!({"protocol": PROTOCOL, "id": id, "command": "move", "point": point_json(&point)})
        }
        Command::Phase(phase) => {
            json!({"protocol": PROTOCOL, "id": id, "command": "phase", "phase": phase.as_str()})
        }
        Command::Hide => json!({"protocol": PROTOCOL, "id": id, "command": "hide"}),
        Command::Query => json!({"protocol": PROTOCOL, "id": id, "command": "query"}),
        Command::Quit => json!({"protocol": PROTOCOL, "id": id, "command": "quit"}),
    }
}

/// Parses one reply line. Any violation yields `invalid_reply`.
pub fn parse_reply(bytes: &[u8]) -> Result<Reply, &'static str> {
    let value = crate::strict_json::from_slice(bytes).map_err(|_| INVALID_REPLY)?;
    let map = value.as_object().ok_or(INVALID_REPLY)?;
    let keys = [
        "protocol",
        "id",
        "status",
        "reason",
        "visible",
        "phase",
        "point",
        "window_binding",
        "action_allowed",
    ];
    if !has_exact_keys(map, &keys) {
        return Err(INVALID_REPLY);
    }
    if map["protocol"].as_str() != Some(PROTOCOL)
        || map["window_binding"].as_str() != Some(WINDOW_BINDING)
        || map["action_allowed"].as_bool() != Some(false)
    {
        return Err(INVALID_REPLY);
    }

    let id = match &map["id"] {
        Value::Null => None,
        other => Some(valid_id(other).ok_or(INVALID_REPLY)?),
    };
    let reason = match (map["status"].as_str(), &map["reason"]) {
        (Some("ok"), Value::Null) => None,
        (Some("error"), Value::String(text)) if (1..=64).contains(&text.len()) => {
            Some(text.clone())
        }
        _ => return Err(INVALID_REPLY),
    };

    let visible = map["visible"].as_bool().ok_or(INVALID_REPLY)?;
    let phase = map["phase"]
        .as_str()
        .and_then(Phase::parse)
        .ok_or(INVALID_REPLY)?;
    let point = match &map["point"] {
        Value::Null => None,
        other => Some(parse_point(other).ok_or(INVALID_REPLY)?),
    };

    let consistent = if visible {
        point.is_some()
    } else {
        point.is_none() && phase == Phase::Idle
    };
    if !consistent {
        return Err(INVALID_REPLY);
    }

    Ok(Reply {
        id,
        reason,
        state: State {
            visible,
            phase,
            point,
        },
    })
}

fn valid_id(value: &Value) -> Option<String> {
    let id = value.as_str()?;
    let ok = (1..=64).contains(&id.len()) && id.bytes().all(|b| (0x21..=0x7e).contains(&b));
    ok.then(|| id.to_string())
}

fn parse_point(value: &Value) -> Option<Point> {
    let map = value.as_object()?;
    if !has_exact_keys(map, &["space", "display_id", "x", "y"]) {
        return None;
    }
    if map["space"].as_str() != Some(SPACE) {
        return None;
    }
    let display_id = u32::try_from(map["display_id"].as_u64()?).ok()?;
    if display_id == 0 {
        return None;
    }
    Some(Point {
        display_id,
        x: finite_coordinate(&map["x"])?,
        y: finite_coordinate(&map["y"])?,
    })
}

fn finite_coordinate(value: &Value) -> Option<f64> {
    if !value.is_number() {
        return None;
    }
    let number = value.as_f64()?;
    (number.is_finite() && number.abs() <= MAX_COORDINATE).then_some(number)
}

fn point_json(point: &Point) -> Value {
    json!({"space": SPACE, "display_id": point.display_id, "x": point.x, "y": point.y})
}

fn has_exact_keys(map: &Map<String, Value>, keys: &[&str]) -> bool {
    map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};
    use std::path::PathBuf;

    const HEAD: &str = r#""protocol":"cursor-overlay-v1","id":"a","#;
    const POINT: &str =
        r#"{"space":"global_display_points_top_left","display_id":1,"x":10.5,"y":20}"#;

    fn line(tail: &str) -> String {
        format!("{{{}{}}}", HEAD, tail)
    }

    fn show(point: &str) -> String {
        line(&format!(r#""command":"show","point":{}"#, point))
    }

    fn parse(text: &str) -> Result<Request, &'static str> {
        parse_request(text.as_bytes())
    }

    fn repo_file(relative: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {relative}: {e}"))
    }

    fn valid_reply() -> Value {
        json!({
            "protocol": PROTOCOL,
            "id": "a",
            "status": "ok",
            "reason": null,
            "visible": true,
            "phase": "hold",
            "point": {"space": SPACE, "display_id": 1, "x": 10.5, "y": 20},
            "window_binding": WINDOW_BINDING,
            "action_allowed": false
        })
    }

    fn reply_bytes(edit: impl FnOnce(&mut Map<String, Value>)) -> Vec<u8> {
        let mut map = valid_reply().as_object().cloned().expect("object");
        edit(&mut map);
        Value::Object(map).to_string().into_bytes()
    }

    #[test]
    fn parses_every_command() {
        let point = Point {
            display_id: 1,
            x: 10.5,
            y: 20.0,
        };
        assert_eq!(
            parse(&line(&format!(r#""command":"show","point":{}"#, POINT))),
            Ok(Request {
                id: "a".to_string(),
                command: Command::Show(point)
            })
        );
        assert_eq!(
            parse(&line(&format!(r#""command":"move","point":{}"#, POINT))),
            Ok(Request {
                id: "a".to_string(),
                command: Command::Move(point)
            })
        );
        assert_eq!(
            parse(&line(r#""command":"phase","phase":"drag""#)),
            Ok(Request {
                id: "a".to_string(),
                command: Command::Phase(Phase::Drag)
            })
        );
        assert_eq!(
            parse(&line(r#""command":"hide""#)).unwrap().command,
            Command::Hide
        );
        assert_eq!(
            parse(&line(r#""command":"query""#)).unwrap().command,
            Command::Query
        );
        assert_eq!(
            parse(&line(r#""command":"quit""#)).unwrap().command,
            Command::Quit
        );
    }

    #[test]
    fn request_json_round_trips() {
        let point = Point {
            display_id: 2,
            x: -3.25,
            y: 1000000.0,
        };
        let commands = [
            Command::Show(point),
            Command::Move(point),
            Command::Phase(Phase::Click),
            Command::Hide,
            Command::Query,
            Command::Quit,
        ];
        for command in commands {
            let request = Request {
                id: "a".to_string(),
                command,
            };
            let text = request_json(&request).to_string();
            assert_eq!(parse(&text), Ok(request));
        }
        assert_eq!(
            request_json(&Request {
                id: "a".to_string(),
                command: Command::Hide
            }),
            json!({"protocol": "cursor-overlay-v1", "id": "a", "command": "hide"})
        );
    }

    #[test]
    fn refuses_invalid_and_unknown_requests() {
        let long_id = format!(
            r#"{{"protocol":"cursor-overlay-v1","id":"{}","command":"hide"}}"#,
            "a".repeat(65)
        );
        let cases: Vec<(String, &'static str)> = vec![
            ("not json".to_string(), "invalid_json"),
            ("[]".to_string(), "invalid_json"),
            (line(r#""command":"hide","id":"b""#), "invalid_json"),
            (
                r#"{"protocol":"cursor-overlay-v0","id":"a","command":"hide"}"#.to_string(),
                "invalid_request",
            ),
            (
                r#"{"protocol":"cursor-overlay-v1","id":"","command":"hide"}"#.to_string(),
                "invalid_request",
            ),
            (
                r#"{"protocol":"cursor-overlay-v1","id":"a b","command":"hide"}"#.to_string(),
                "invalid_request",
            ),
            (long_id, "invalid_request"),
            (
                r#"{"protocol":"cursor-overlay-v1","id":"a","command":1}"#.to_string(),
                "invalid_request",
            ),
            (line(r#""command":"hide","extra":1"#), "invalid_request"),
            (line(r#""command":"show""#), "invalid_request"),
            (
                line(&format!(r#""command":"hide","point":{}"#, POINT)),
                "invalid_request",
            ),
            (line(r#""command":"click""#), "unknown_command"),
            (
                line(r#""command":"phase","phase":"pressed""#),
                "invalid_phase",
            ),
            (line(r#""command":"phase","phase":1"#), "invalid_phase"),
            (
                show(r#"{"space":"window","display_id":1,"x":1,"y":1}"#),
                "invalid_point",
            ),
            (
                show(r#"{"space":"global_display_points_top_left","display_id":0,"x":1,"y":1}"#),
                "invalid_point",
            ),
            (
                show(r#"{"space":"global_display_points_top_left","display_id":1.5,"x":1,"y":1}"#),
                "invalid_point",
            ),
            (
                show(
                    r#"{"space":"global_display_points_top_left","display_id":4294967296,"x":1,"y":1}"#,
                ),
                "invalid_point",
            ),
            (
                show(r#"{"space":"global_display_points_top_left","display_id":1,"x":true,"y":1}"#),
                "invalid_point",
            ),
            (
                show(r#"{"space":"global_display_points_top_left","display_id":1,"x":"1","y":1}"#),
                "invalid_point",
            ),
            (
                show(
                    r#"{"space":"global_display_points_top_left","display_id":1,"x":1000001,"y":1}"#,
                ),
                "invalid_point",
            ),
            (
                show(
                    r#"{"space":"global_display_points_top_left","display_id":1,"x":1,"y":-1000001}"#,
                ),
                "invalid_point",
            ),
            (
                show(
                    r#"{"space":"global_display_points_top_left","display_id":1,"x":1,"y":1,"z":0}"#,
                ),
                "invalid_point",
            ),
            (
                show(r#"{"space":"global_display_points_top_left","display_id":1,"x":1}"#),
                "invalid_point",
            ),
        ];
        for (text, reason) in cases {
            assert_eq!(parse(&text), Err(reason), "input: {text}");
        }
    }

    #[test]
    fn accepts_boundary_values() {
        let x_max = show(
            r#"{"space":"global_display_points_top_left","display_id":4294967295,"x":1000000,"y":-1000000}"#,
        );
        assert!(parse(&x_max).is_ok());
        let long_id = format!(
            r#"{{"protocol":"cursor-overlay-v1","id":"{}","command":"hide"}}"#,
            "a".repeat(64)
        );
        assert!(parse(&long_id).is_ok());
    }

    #[test]
    fn parses_valid_replies() {
        let reply = parse_reply(valid_reply().to_string().as_bytes()).expect("valid reply");
        assert_eq!(
            reply,
            Reply {
                id: Some("a".to_string()),
                reason: None,
                state: State {
                    visible: true,
                    phase: Phase::Hold,
                    point: Some(Point {
                        display_id: 1,
                        x: 10.5,
                        y: 20.0
                    }),
                },
            }
        );
        assert!(!reply.action_allowed());

        let error = json!({
            "protocol": "cursor-overlay-v1",
            "id": null,
            "status": "error",
            "reason": "invalid_json",
            "visible": false,
            "phase": "idle",
            "point": null,
            "window_binding": "unbound",
            "action_allowed": false
        });
        assert_eq!(
            parse_reply(error.to_string().as_bytes()),
            Ok(Reply {
                id: None,
                reason: Some("invalid_json".to_string()),
                state: State {
                    visible: false,
                    phase: Phase::Idle,
                    point: None
                },
            })
        );
    }

    #[test]
    fn refuses_invalid_replies() {
        let invalid = "invalid_reply";
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("action_allowed".into(), json!(true));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("window_binding".into(), json!("window"));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("protocol".into(), json!("cursor-overlay-v0"));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.remove("action_allowed");
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("extra".into(), json!(1));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("reason".into(), json!("x"));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("status".into(), json!("error"));
                m.insert("reason".into(), json!(null));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("status".into(), json!("done"));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("phase".into(), json!("pressed"));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("visible".into(), json!(1));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("visible".into(), json!(true));
                m.insert("point".into(), json!(null));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("visible".into(), json!(false));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("visible".into(), json!(false));
                m.insert("point".into(), json!(null));
                m.insert("phase".into(), json!("hold"));
            })),
            Err(invalid)
        );
        assert_eq!(
            parse_reply(&reply_bytes(|m| {
                m.insert("id".into(), json!(""));
            })),
            Err(invalid)
        );
        assert_eq!(parse_reply(b"not json"), Err(invalid));
    }

    #[test]
    fn helper_source_is_display_only() {
        let source = repo_file("CursorOverlay.swift");
        let imports: Vec<&str> = source
            .lines()
            .filter(|l| l.starts_with("import "))
            .collect();
        assert_eq!(imports, vec!["import AppKit"]);

        for forbidden in [
            "CGEvent",
            "CGWarpMouseCursorPosition",
            "CGAssociateMouseAndMouseCursorPosition",
            "CGDisplayMoveCursorToPoint",
            "AXUIElement",
            "AXIsProcessTrusted",
            "IOHID",
            "CGPreflight",
            "CGRequest",
            "CGWindowList",
            "SCShareableContent",
            "NSCursor",
            "NSEvent",
            "activate(",
            "makeKey",
            "makeMain",
        ] {
            assert!(
                !source.contains(forbidden),
                "helper must not contain {forbidden}"
            );
        }

        for required in [
            "panel.ignoresMouseEvents = true",
            ".nonactivatingPanel",
            "override var canBecomeKey: Bool { false }",
            "override var canBecomeMain: Bool { false }",
            "setActivationPolicy(.accessory)",
            "orderFrontRegardless()",
            "\"window_binding\": \"unbound\"",
            "\"action_allowed\": false",
            "!panel.isVisible",
            "didChangeScreenParametersNotification",
            "reason: \"display_changed\"",
        ] {
            assert!(source.contains(required), "helper must contain {required}");
        }

        assert!(source.contains(&format!("\"{}\"", PROTOCOL)));
        assert!(source.contains(&format!("\"{}\"", SPACE)));
    }

    #[test]
    fn helper_is_built_and_self_tested_in_ci() {
        let script = repo_file("build-cursor-overlay.sh");
        assert!(script.contains("CursorOverlay.swift -o target/debug/mac-cursor-overlay"));
        assert!(script.contains("target/debug/mac-cursor-overlay --self-test"));
        let workflow = repo_file(".github/workflows/ci.yml");
        assert!(workflow.contains("      - run: sh build-cursor-overlay.sh"));
    }

    #[test]
    fn doc_states_display_is_not_authorization() {
        let doc = repo_file("docs/CURSOR-OVERLAY.md");
        for required in [
            "cursor-overlay-v1",
            "global_display_points_top_left",
            "不授權輸入",
            "未綁定視窗",
            "action_allowed",
            "Accessibility",
            "Input Monitoring",
            "Screen Recording",
            "not_visible",
            "point_outside_display",
        ] {
            assert!(doc.contains(required), "doc must contain {required}");
        }
        assert!(repo_file("docs/BUILD.md").contains("build-cursor-overlay.sh"));
    }
}
