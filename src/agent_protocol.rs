//! Versioned request/response protocol of the ReachAgent service.
//!
//! Platform independent: it only parses and builds JSON lines. The service itself
//! (`ReachAgent.swift`) speaks the same protocol over a Unix domain socket.
//! Nothing here asks for a permission or posts an input event.
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: u32 = 1;
/// Longest accepted request line, in bytes, without the trailing newline.
pub const MAX_LINE_BYTES: usize = 65536;
pub const METHODS: [&str; 11] = [
    "describe",
    "permissions",
    "request_permissions",
    "capture",
    "capture_all_windows",
    "accessibility",
    "accessibility_subtree",
    "input_source",
    "secure_input",
    "pointer_location",
    "bundle_windows",
];
/// The read-only observation methods (plan 4.5, I-3). They need no permission, send no event
/// and are not used by the input gate.
pub const OBSERVATION_METHODS: [&str; 4] = [
    "input_source",
    "secure_input",
    "pointer_location",
    "bundle_windows",
];
/// Most distinct owner processes one `bundle_windows` report may list (the Swift
/// `maxObservedProcesses`).
pub const MAX_OBSERVED_PROCESSES: usize = 8;
/// Most windows one `bundle_windows` report may list (the Swift `maxObservedWindows`).
pub const MAX_OBSERVED_WINDOWS: usize = 64;
/// Longest accepted `image` path of a capture request, in bytes.
pub const MAX_IMAGE_PATH_BYTES: usize = 1024;
/// Largest image side, in pixels, that a capture may report.
const MAX_IMAGE_SIDE: u64 = 8192;
/// Most nodes one accessibility report may count (the Swift `maxNodes`).
const MAX_AX_NODES: u64 = 160;
/// Most windows one accessibility report may list (the Swift `maxWindows`).
const MAX_AX_WINDOWS: usize = 32;
/// Longest accepted subtree `path`.
const MAX_SUBTREE_PATH: usize = 8;
pub const PERMISSIONS: [&str; 3] = ["accessibility", "screen_recording", "post_event"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
}

impl PermissionState {
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionState::Granted => "granted",
            PermissionState::Denied => "denied",
            PermissionState::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Accessibility,
    ScreenRecording,
    PostEvent,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Accessibility => "accessibility",
            Permission::ScreenRecording => "screen_recording",
            Permission::PostEvent => "post_event",
        }
    }

    pub fn parse(name: &str) -> Option<Permission> {
        match name {
            "accessibility" => Some(Permission::Accessibility),
            "screen_recording" => Some(Permission::ScreenRecording),
            "post_event" => Some(Permission::PostEvent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Describe,
    Permissions,
    RequestPermissions,
    Capture,
    CaptureAllWindows,
    Accessibility,
    AccessibilitySubtree,
    InputSource,
    SecureInput,
    PointerLocation,
    BundleWindows,
}

impl Method {
    pub fn parse(name: &str) -> Option<Method> {
        match name {
            "describe" => Some(Method::Describe),
            "permissions" => Some(Method::Permissions),
            "request_permissions" => Some(Method::RequestPermissions),
            "capture" => Some(Method::Capture),
            "capture_all_windows" => Some(Method::CaptureAllWindows),
            "accessibility" => Some(Method::Accessibility),
            "accessibility_subtree" => Some(Method::AccessibilitySubtree),
            "input_source" => Some(Method::InputSource),
            "secure_input" => Some(Method::SecureInput),
            "pointer_location" => Some(Method::PointerLocation),
            "bundle_windows" => Some(Method::BundleWindows),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionSet {
    pub accessibility: PermissionState,
    pub screen_recording: PermissionState,
    pub post_event: PermissionState,
}

impl PermissionSet {
    pub fn get(&self, p: Permission) -> PermissionState {
        match p {
            Permission::Accessibility => self.accessibility,
            Permission::ScreenRecording => self.screen_recording,
            Permission::PostEvent => self.post_event,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "accessibility": self.accessibility.as_str(),
            "screen_recording": self.screen_recording.as_str(),
            "post_event": self.post_event.as_str(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentError {
    pub code: &'static str,
    pub message: String,
    pub permission: Option<(Permission, PermissionState)>,
}

impl AgentError {
    fn new(code: &'static str, message: impl Into<String>) -> AgentError {
        AgentError {
            code,
            message: message.into(),
            permission: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    pub method: Method,
    pub params: Value,
}

type ParseFailure = (Option<String>, AgentError);

/// Parse one request line (without its trailing newline).
/// On failure the first tuple item is the `id` when it could be read.
pub fn parse_request(line: &str) -> Result<Request, ParseFailure> {
    let fail = |id: Option<String>, code: &'static str, message: &str| -> ParseFailure {
        (id, AgentError::new(code, message))
    };
    if line.len() > MAX_LINE_BYTES {
        return Err(fail(None, "invalid_json", "request line is too long"));
    }
    if line.contains('\n') || line.contains('\r') {
        return Err(fail(
            None,
            "invalid_json",
            "a request must be exactly one line",
        ));
    }
    let value = crate::strict_json::from_slice(line.as_bytes())
        .map_err(|e| fail(None, "invalid_json", &format!("not valid JSON: {e}")))?;
    let Some(object) = value.as_object() else {
        return Err(fail(
            None,
            "invalid_json",
            "the request must be a JSON object",
        ));
    };
    let id = object.get("id").and_then(Value::as_str).map(str::to_string);
    let Some(request_id) = id.clone() else {
        return Err(fail(None, "missing_field", "`id` must be a string"));
    };
    let Some(version) = object.get("version").and_then(Value::as_u64) else {
        return Err(fail(
            id,
            "missing_field",
            "`version` must be a non-negative integer",
        ));
    };
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Err(fail(id, "missing_field", "`method` must be a string"));
    };
    if version != u64::from(PROTOCOL_VERSION) {
        return Err(fail(
            id,
            "version_mismatch",
            &format!("this service speaks protocol version {PROTOCOL_VERSION}"),
        ));
    }
    let Some(method) = Method::parse(method) else {
        return Err(fail(
            id,
            "unknown_method",
            &format!("unknown method `{method}`"),
        ));
    };
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    Ok(Request {
        id: request_id,
        method,
        params,
    })
}

/// Only the same user may talk to the service.
pub fn check_peer(peer_uid: u32, own_uid: u32) -> Result<(), AgentError> {
    if peer_uid == own_uid {
        Ok(())
    } else {
        Err(AgentError::new(
            "peer_uid_mismatch",
            "the peer is not the user that runs the service",
        ))
    }
}

/// A method that needs a permission may run only when it is `granted`.
pub fn require_permission(set: &PermissionSet, needed: Permission) -> Result<(), AgentError> {
    let state = set.get(needed);
    if state == PermissionState::Granted {
        return Ok(());
    }
    Err(AgentError {
        code: "permission_denied",
        message: format!("{} is {}", needed.as_str(), state.as_str()),
        permission: Some((needed, state)),
    })
}

/// The permissions named in the params of `request_permissions`.
pub fn parse_requested(params: &Value) -> Result<Vec<Permission>, AgentError> {
    let invalid = |message: &str| AgentError::new("invalid_params", message);
    let list = params
        .get("permissions")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("`permissions` must be an array"))?;
    if list.is_empty() {
        return Err(invalid("`permissions` must not be empty"));
    }
    let mut requested = Vec::new();
    for item in list {
        let name = item
            .as_str()
            .ok_or_else(|| invalid("every permission name must be a string"))?;
        let permission =
            Permission::parse(name).ok_or_else(|| invalid("unknown permission name"))?;
        if requested.contains(&permission) {
            return Err(invalid("a permission name appears twice"));
        }
        requested.push(permission);
    }
    if requested.contains(&Permission::PostEvent) {
        return Err(AgentError::new(
            "not_requestable",
            "post_event cannot be requested: input stays disabled",
        ));
    }
    Ok(requested)
}

/// The validated params of `capture` and `capture_all_windows`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureParams {
    pub pid: i32,
    pub window_id: u32,
    pub image: String,
}

/// Exactly `pid`, `window_id` and `image`; the integers must be JSON integers in range and
/// `image` an absolute `.png` path. The Swift `parseCaptureParams` follows the same rules.
pub fn parse_capture_params(params: &Value) -> Result<CaptureParams, AgentError> {
    let invalid = |message: &str| AgentError::new("invalid_params", message);
    let object = params
        .as_object()
        .ok_or_else(|| invalid("params must be an object"))?;
    if let Some(extra) = object
        .keys()
        .find(|k| !["pid", "window_id", "image"].contains(&k.as_str()))
    {
        return Err(invalid(&format!("unknown parameter `{extra}`")));
    }
    let pid = object
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|n| i32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("`pid` must be a positive 32-bit integer"))?;
    let window_id = object
        .get("window_id")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("`window_id` must be a positive 32-bit integer"))?;
    let image = object
        .get("image")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("`image` must be a string"))?;
    if !image.starts_with('/')
        || !image.ends_with(".png")
        || image.len() > MAX_IMAGE_PATH_BYTES
        || image.contains('\0')
    {
        return Err(invalid(
            "`image` must be an absolute .png path of at most 1024 bytes",
        ));
    }
    Ok(CaptureParams {
        pid,
        window_id,
        image: image.to_string(),
    })
}

/// Check a capture result against the request that produced it. The first mismatch is
/// reported as `<field>: <reason>`. `image_sha256` is not part of the result; the caller
/// computes it from the file.
pub fn check_capture_result(
    method: Method,
    params: &CaptureParams,
    result: &Value,
) -> Result<(), String> {
    let expected_scope = match method {
        Method::Capture => "onscreen_candidates",
        Method::CaptureAllWindows => "all_window_candidates",
        _ => return Err("method: not a capture method".to_string()),
    };
    let object = result
        .as_object()
        .ok_or_else(|| "result: not an object".to_string())?;
    if object.get("window_id").and_then(Value::as_u64) != Some(u64::from(params.window_id)) {
        return Err("window_id: differs from the request".to_string());
    }
    if object.get("owner_pid").and_then(Value::as_i64) != Some(i64::from(params.pid)) {
        return Err("owner_pid: differs from the request".to_string());
    }
    if object.get("image_path").and_then(Value::as_str) != Some(params.image.as_str()) {
        return Err("image_path: differs from the request".to_string());
    }
    for side in ["image_width", "image_height"] {
        match object.get(side).and_then(Value::as_u64) {
            Some(n) if (1..=MAX_IMAGE_SIDE).contains(&n) => {}
            _ => {
                return Err(format!(
                    "{side}: must be an integer from 1 to {MAX_IMAGE_SIDE}"
                ))
            }
        }
    }
    if object.get("capture_type").and_then(Value::as_str) != Some("desktop_independent_window") {
        return Err("capture_type: must be desktop_independent_window".to_string());
    }
    if object.get("window_scope").and_then(Value::as_str) != Some(expected_scope) {
        return Err(format!("window_scope: must be {expected_scope}"));
    }
    for flag in ["onscreen_before", "onscreen_after"] {
        if !object.get(flag).is_some_and(Value::is_boolean) {
            return Err(format!("{flag}: must be a boolean"));
        }
    }
    if object.get("occlusion_checked") != Some(&Value::Bool(false)) {
        return Err("occlusion_checked: must be false".to_string());
    }
    let frame = object
        .get("window_frame_points")
        .and_then(Value::as_object)
        .ok_or_else(|| "window_frame_points: must be an object".to_string())?;
    for key in ["x", "y", "width", "height"] {
        match frame.get(key).and_then(Value::as_f64) {
            Some(n) if n.is_finite() && (!matches!(key, "width" | "height") || n > 0.0) => {}
            _ => {
                return Err(format!(
                    "window_frame_points.{key}: must be a finite number"
                ))
            }
        }
    }
    Ok(())
}

/// A positive 32-bit process id from `object["pid"]`.
fn positive_pid(object: &serde_json::Map<String, Value>) -> Result<i32, AgentError> {
    object
        .get("pid")
        .and_then(Value::as_i64)
        .and_then(|n| i32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| AgentError::new("invalid_params", "`pid` must be a positive 32-bit integer"))
}

/// The validated params of `accessibility`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessibilityParams {
    pub pid: i32,
    pub windows_only: bool,
}

/// `pid` is required; `windows_only` is an optional boolean (default false). Nothing else.
/// The Swift `parseAccessibilityParams` follows the same rules.
pub fn parse_accessibility_params(params: &Value) -> Result<AccessibilityParams, AgentError> {
    let invalid = |message: &str| AgentError::new("invalid_params", message);
    let object = params
        .as_object()
        .ok_or_else(|| invalid("params must be an object"))?;
    if let Some(extra) = object
        .keys()
        .find(|k| !["pid", "windows_only"].contains(&k.as_str()))
    {
        return Err(invalid(&format!("unknown parameter `{extra}`")));
    }
    let pid = positive_pid(object)?;
    let windows_only = match object.get("windows_only") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => return Err(invalid("`windows_only` must be a boolean")),
    };
    Ok(AccessibilityParams { pid, windows_only })
}

/// The validated params of `accessibility_subtree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreeParams {
    pub pid: i32,
    pub source: String,
    pub path: Vec<u8>,
}

/// Exactly `pid`, `source` (`main`, `focused` or `windows`) and `path` (at most 8 child
/// indexes from 0 to 159; `windows` needs a first index from 0 to 31).
/// The Swift `parseSubtreeParams` follows the same rules.
pub fn parse_subtree_params(params: &Value) -> Result<SubtreeParams, AgentError> {
    let invalid = |message: &str| AgentError::new("invalid_params", message);
    let object = params
        .as_object()
        .ok_or_else(|| invalid("params must be an object"))?;
    if let Some(extra) = object
        .keys()
        .find(|k| !["pid", "source", "path"].contains(&k.as_str()))
    {
        return Err(invalid(&format!("unknown parameter `{extra}`")));
    }
    let pid = positive_pid(object)?;
    let source = match object.get("source").and_then(Value::as_str) {
        Some(s @ ("main" | "focused" | "windows")) => s,
        _ => return Err(invalid("`source` must be main, focused or windows")),
    };
    let items = object
        .get("path")
        .and_then(Value::as_array)
        .filter(|a| a.len() <= MAX_SUBTREE_PATH)
        .ok_or_else(|| invalid("`path` must be an array of at most 8 integers"))?;
    let mut path = Vec::with_capacity(items.len());
    for item in items {
        let index = item
            .as_u64()
            .filter(|n| *n < MAX_AX_NODES)
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| invalid("`path` items must be integers from 0 to 159"))?;
        path.push(index);
    }
    if source == "windows" {
        match path.first() {
            Some(first) if usize::from(*first) < MAX_AX_WINDOWS => {}
            _ => {
                return Err(invalid(
                    "`path` for source windows must start with a window index from 0 to 31",
                ))
            }
        }
    }
    Ok(SubtreeParams {
        pid,
        source: source.to_string(),
        path,
    })
}

/// Check an `accessibility` result against the request. The first mismatch is reported as
/// `<field>: <reason>`.
pub fn check_accessibility_result(
    params: &AccessibilityParams,
    result: &Value,
) -> Result<(), String> {
    let object = result
        .as_object()
        .ok_or_else(|| "result: not an object".to_string())?;
    check_ax_common(object, params.pid)?;
    let expected_tree = if params.windows_only {
        "windows_only"
    } else {
        "full_bounded_tree"
    };
    if object.get("scope").and_then(Value::as_str) != Some("process_ax_windows") {
        return Err("scope: must be process_ax_windows".to_string());
    }
    if object.get("tree_scope").and_then(Value::as_str) != Some(expected_tree) {
        return Err(format!("tree_scope: must be {expected_tree}"));
    }
    if object
        .get("window_count_reported")
        .and_then(Value::as_u64)
        .is_none()
    {
        return Err("window_count_reported: must be a non-negative integer".to_string());
    }
    match object.get("node_count").and_then(Value::as_u64) {
        Some(n) if n <= MAX_AX_NODES => {}
        _ => {
            return Err(format!(
                "node_count: must be an integer from 0 to {MAX_AX_NODES}"
            ))
        }
    }
    match object.get("windows").and_then(Value::as_array) {
        Some(windows) if windows.len() <= MAX_AX_WINDOWS => {}
        _ => {
            return Err(format!(
                "windows: must be an array of at most {MAX_AX_WINDOWS} items"
            ))
        }
    }
    match object.get("fallback") {
        Some(Value::Null | Value::Object(_)) => {}
        _ => return Err("fallback: must be null or an object".to_string()),
    }
    Ok(())
}

/// Check an `accessibility_subtree` result against the request.
pub fn check_subtree_result(params: &SubtreeParams, result: &Value) -> Result<(), String> {
    let object = result
        .as_object()
        .ok_or_else(|| "result: not an object".to_string())?;
    check_ax_common(object, params.pid)?;
    if object.get("source").and_then(Value::as_str) != Some(params.source.as_str()) {
        return Err("source: differs from the request".to_string());
    }
    let expected_path: Vec<u64> = params.path.iter().map(|n| u64::from(*n)).collect();
    let path: Option<Vec<u64>> = object
        .get("path")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(Value::as_u64).collect());
    if path.as_deref() != Some(expected_path.as_slice()) {
        return Err("path: differs from the request".to_string());
    }
    if !object.get("node").is_some_and(Value::is_object) {
        return Err("node: must be an object".to_string());
    }
    match object.get("node_count").and_then(Value::as_u64) {
        Some(n) if (1..=MAX_AX_NODES).contains(&n) => {}
        _ => {
            return Err(format!(
                "node_count: must be an integer from 1 to {MAX_AX_NODES}"
            ))
        }
    }
    if object.get("scope").and_then(Value::as_str) != Some("read_only_subtree_unverified") {
        return Err("scope: must be read_only_subtree_unverified".to_string());
    }
    Ok(())
}

/// The fields every accessibility report shares.
fn check_ax_common(object: &serde_json::Map<String, Value>, pid: i32) -> Result<(), String> {
    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("schema_version: must be 1".to_string());
    }
    if object.get("status").and_then(Value::as_str) != Some("ok") {
        return Err("status: must be ok".to_string());
    }
    if object.get("pid").and_then(Value::as_i64) != Some(i64::from(pid)) {
        return Err("pid: differs from the request".to_string());
    }
    if object.get("action_allowed") != Some(&Value::Bool(false)) {
        return Err("action_allowed: must be false".to_string());
    }
    if object.get("cg_window_binding").and_then(Value::as_str) != Some("unverified") {
        return Err("cg_window_binding: must be unverified".to_string());
    }
    if !object.get("truncated").is_some_and(Value::is_boolean) {
        return Err("truncated: must be a boolean".to_string());
    }
    Ok(())
}

/// `input_source`, `secure_input` and `pointer_location` take no parameters: `params` is
/// absent (`None`) or an empty object. The Swift `parseNoObservationParams` follows the same
/// rules.
pub fn parse_no_observation_params(params: Option<&Value>) -> Result<(), AgentError> {
    let invalid = |message: &str| AgentError::new("invalid_params", message);
    let Some(params) = params else {
        return Ok(());
    };
    let object = params
        .as_object()
        .ok_or_else(|| invalid("params must be an object"))?;
    match object.keys().min() {
        Some(extra) => Err(invalid(&format!("unknown parameter `{extra}`"))),
        None => Ok(()),
    }
}

/// 1 to 255 bytes of ASCII letters, digits, dots and hyphens.
pub fn valid_observed_bundle_id(value: &str) -> bool {
    (1..=255).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// Exactly `bundle_id`. The Swift `parseBundleWindowsParams` follows the same rules.
pub fn parse_bundle_windows_params(params: &Value) -> Result<String, AgentError> {
    let invalid = |message: &str| AgentError::new("invalid_params", message);
    let object = params
        .as_object()
        .ok_or_else(|| invalid("params must be an object"))?;
    if let Some(extra) = object.keys().filter(|k| *k != "bundle_id").min() {
        return Err(invalid(&format!("unknown parameter `{extra}`")));
    }
    match object.get("bundle_id").and_then(Value::as_str) {
        Some(id) if valid_observed_bundle_id(id) => Ok(id.to_string()),
        _ => Err(invalid(
            "`bundle_id` must be 1 to 255 bytes of ASCII letters, digits, dots and hyphens",
        )),
    }
}

fn finite_number(object: &serde_json::Map<String, Value>, field: &str) -> Result<f64, String> {
    object
        .get(field)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("{field}: must be a finite number"))
}

fn nullable_text(object: &serde_json::Map<String, Value>, field: &str) -> Result<(), String> {
    match object.get(field) {
        Some(Value::Null | Value::String(_)) => Ok(()),
        _ => Err(format!("{field}: must be a string or null")),
    }
}

/// Check the result of a read-only observation. `bundle_id` is the request's for
/// `bundle_windows` and ignored otherwise. Every observation names its backend, says it is
/// read-only and unverified, and is never used by the input gate.
pub fn check_observation_result(
    method: Method,
    bundle_id: Option<&str>,
    result: &Value,
) -> Result<(), String> {
    let object = result
        .as_object()
        .ok_or_else(|| "result: not an object".to_string())?;
    if !matches!(
        object.get("backend").and_then(Value::as_str),
        Some("fake" | "system")
    ) {
        return Err("backend: must be fake or system".to_string());
    }
    if object.get("scope").and_then(Value::as_str) != Some("read_only_unverified") {
        return Err("scope: must be read_only_unverified".to_string());
    }
    if object.get("action_allowed") != Some(&Value::Bool(false)) {
        return Err("action_allowed: must be false".to_string());
    }
    if object.get("used_by_input_gate") != Some(&Value::Bool(false)) {
        return Err("used_by_input_gate: must be false".to_string());
    }
    match method {
        Method::InputSource => {
            match object.get("input_source_id").and_then(Value::as_str) {
                Some(id) if !id.is_empty() => {}
                _ => return Err("input_source_id: must be a non-empty string".to_string()),
            }
            for field in ["input_mode_id", "localized_name", "source_type"] {
                nullable_text(object, field)?;
            }
            if object.get("selection_changed") != Some(&Value::Bool(false)) {
                return Err("selection_changed: must be false".to_string());
            }
        }
        Method::SecureInput => {
            if !object
                .get("secure_event_input_enabled")
                .is_some_and(Value::is_boolean)
            {
                return Err("secure_event_input_enabled: must be a boolean".to_string());
            }
        }
        Method::PointerLocation => {
            finite_number(object, "x")?;
            finite_number(object, "y")?;
            if object.get("coordinate_space").and_then(Value::as_str)
                != Some("global_top_left_points")
            {
                return Err("coordinate_space: must be global_top_left_points".to_string());
            }
        }
        Method::BundleWindows => check_bundle_windows(object, bundle_id)?,
        _ => return Err("method: not an observation method".to_string()),
    }
    Ok(())
}

fn check_bundle_windows(
    object: &serde_json::Map<String, Value>,
    bundle_id: Option<&str>,
) -> Result<(), String> {
    if bundle_id.is_none() || object.get("bundle_id").and_then(Value::as_str) != bundle_id {
        return Err("bundle_id: differs from the request".to_string());
    }
    if object.get("window_scope").and_then(Value::as_str) != Some("onscreen_only") {
        return Err("window_scope: must be onscreen_only".to_string());
    }
    if object.get("window_titles").and_then(Value::as_str) != Some("not_read") {
        return Err("window_titles: must be not_read".to_string());
    }
    if !object.get("truncated").is_some_and(Value::is_boolean) {
        return Err("truncated: must be a boolean".to_string());
    }
    let owners: Vec<i64> = match object.get("owner_pids").and_then(Value::as_array) {
        Some(list) if list.len() <= MAX_OBSERVED_PROCESSES => list
            .iter()
            .map(|v| v.as_i64().filter(|n| (1..=i64::from(i32::MAX)).contains(n)))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "owner_pids: must be positive 32-bit integers".to_string())?,
        _ => {
            return Err(format!(
                "owner_pids: must be an array of at most {MAX_OBSERVED_PROCESSES} items"
            ))
        }
    };
    let windows = match object.get("windows").and_then(Value::as_array) {
        Some(list) if list.len() <= MAX_OBSERVED_WINDOWS => list,
        _ => {
            return Err(format!(
                "windows: must be an array of at most {MAX_OBSERVED_WINDOWS} items"
            ))
        }
    };
    for window in windows {
        let window = window
            .as_object()
            .ok_or_else(|| "windows: every item must be an object".to_string())?;
        match window.get("window_id").and_then(Value::as_u64) {
            Some(n) if (1..=u64::from(u32::MAX)).contains(&n) => {}
            _ => return Err("windows.window_id: must be a positive 32-bit integer".to_string()),
        }
        match window.get("owner_pid").and_then(Value::as_i64) {
            Some(pid) if owners.contains(&pid) => {}
            _ => return Err("windows.owner_pid: must be one of owner_pids".to_string()),
        }
        if window.get("layer").and_then(Value::as_i64).is_none() {
            return Err("windows.layer: must be an integer".to_string());
        }
        if window.get("z_index").and_then(Value::as_u64).is_none() {
            return Err("windows.z_index: must be a non-negative integer".to_string());
        }
        finite_number(window, "alpha").map_err(|e| format!("windows.{e}"))?;
        let bounds = window
            .get("bounds")
            .and_then(Value::as_object)
            .ok_or_else(|| "windows.bounds: must be an object".to_string())?;
        for field in ["x", "y", "width", "height"] {
            finite_number(bounds, field).map_err(|e| format!("windows.bounds.{e}"))?;
        }
    }
    Ok(())
}

pub fn describe_result(app_version: &str, test_mode: bool) -> Value {
    json!({
        "app": "ReachAgent",
        "app_version": app_version,
        "protocol_version": PROTOCOL_VERSION,
        "methods": METHODS,
        "test_mode": test_mode,
    })
}

pub fn ok_response(id: &str, result: Value) -> Value {
    json!({"version": PROTOCOL_VERSION, "id": id, "ok": true, "result": result})
}

pub fn error_response(id: Option<&str>, err: &AgentError) -> Value {
    let mut error = json!({"code": err.code, "message": err.message});
    if let Some((permission, state)) = err.permission {
        error["permission"] = json!(permission.as_str());
        error["state"] = json!(state.as_str());
    }
    json!({"version": PROTOCOL_VERSION, "id": id, "ok": false, "error": error})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(a: PermissionState, s: PermissionState, p: PermissionState) -> PermissionSet {
        PermissionSet {
            accessibility: a,
            screen_recording: s,
            post_event: p,
        }
    }

    #[test]
    fn parse_request_accepts_describe() {
        let r = parse_request(r#"{"version":1,"id":"a","method":"describe"}"#).expect("valid");
        assert_eq!(r.id, "a");
        assert_eq!(r.method, Method::Describe);
        assert_eq!(r.params, json!({}));
    }

    #[test]
    fn parse_request_rejects_each_invalid_shape() {
        let cases: [(&str, &str, Option<&str>); 11] = [
            ("not json", "invalid_json", None),
            ("[]", "invalid_json", None),
            (
                r#"{"version":1,"version":1,"id":"a","method":"describe"}"#,
                "invalid_json",
                None,
            ),
            (
                "{\"version\":1,\"id\":\"a\",\n\"method\":\"describe\"}",
                "invalid_json",
                None,
            ),
            (
                r#"{"id":"a","method":"describe"}"#,
                "missing_field",
                Some("a"),
            ),
            (
                r#"{"version":1,"method":"describe"}"#,
                "missing_field",
                None,
            ),
            (r#"{"version":1,"id":"a"}"#, "missing_field", Some("a")),
            (
                r#"{"version":"1","id":"a","method":"describe"}"#,
                "missing_field",
                Some("a"),
            ),
            (
                r#"{"version":1,"id":1,"method":"describe"}"#,
                "missing_field",
                None,
            ),
            (
                r#"{"version":2,"id":"a","method":"describe"}"#,
                "version_mismatch",
                Some("a"),
            ),
            (
                r#"{"version":1,"id":"a","method":"nope"}"#,
                "unknown_method",
                Some("a"),
            ),
        ];
        for (line, code, id) in cases {
            let (got_id, err) = parse_request(line).expect_err(line);
            assert_eq!(err.code, code, "{line}");
            assert_eq!(got_id.as_deref(), id, "{line}");
        }
    }

    #[test]
    fn line_over_the_limit_is_invalid_json() {
        let line = "x".repeat(MAX_LINE_BYTES + 1);
        let (_, err) = parse_request(&line).expect_err("too long");
        assert_eq!(err.code, "invalid_json");
    }

    #[test]
    fn check_peer_rejects_other_uid() {
        assert!(check_peer(501, 501).is_ok());
        assert_eq!(
            check_peer(0, 501).expect_err("other uid").code,
            "peer_uid_mismatch"
        );
    }

    #[test]
    fn require_permission_only_accepts_granted() {
        use PermissionState::*;
        let granted = set(Granted, Granted, Granted);
        assert!(require_permission(&granted, Permission::ScreenRecording).is_ok());
        for state in [Denied, Unknown] {
            let s = set(Granted, state, Granted);
            let err = require_permission(&s, Permission::ScreenRecording).expect_err("not granted");
            assert_eq!(err.code, "permission_denied");
            assert_eq!(err.permission, Some((Permission::ScreenRecording, state)));
        }
    }

    #[test]
    fn parse_requested_rules() {
        assert_eq!(
            parse_requested(&json!({"permissions": ["accessibility", "screen_recording"]}))
                .expect("valid"),
            vec![Permission::Accessibility, Permission::ScreenRecording]
        );
        let invalid = [
            json!({"permissions": []}),
            json!({}),
            json!({"permissions": "accessibility"}),
            json!({"permissions": [1]}),
            json!({"permissions": ["nope"]}),
            json!({"permissions": ["accessibility", "accessibility"]}),
        ];
        for params in invalid {
            assert_eq!(
                parse_requested(&params).expect_err("invalid").code,
                "invalid_params",
                "{params}"
            );
        }
        for params in [
            json!({"permissions": ["post_event"]}),
            json!({"permissions": ["accessibility", "post_event"]}),
        ] {
            assert_eq!(
                parse_requested(&params).expect_err("not requestable").code,
                "not_requestable",
                "{params}"
            );
        }
    }

    #[test]
    fn describe_and_responses_have_the_wire_shape() {
        let d = describe_result("0.1.0", true);
        assert_eq!(d["app"], "ReachAgent");
        assert_eq!(d["app_version"], "0.1.0");
        assert_eq!(d["protocol_version"], 1);
        assert_eq!(d["methods"], json!(METHODS));
        assert_eq!(d["test_mode"], true);

        let ok = ok_response("a", json!({"x": 1}));
        assert_eq!(ok["version"], 1);
        assert_eq!(ok["id"], "a");
        assert_eq!(ok["ok"], true);
        assert_eq!(ok["result"], json!({"x": 1}));

        let err = AgentError::new("unknown_method", "no");
        let e = error_response(None, &err);
        assert_eq!(e["id"], Value::Null);
        assert_eq!(e["ok"], false);
        assert_eq!(e["error"]["code"], "unknown_method");
        assert!(e["error"].get("permission").is_none());

        let denied = require_permission(
            &set(
                PermissionState::Denied,
                PermissionState::Denied,
                PermissionState::Denied,
            ),
            Permission::Accessibility,
        )
        .expect_err("denied");
        let e = error_response(Some("b"), &denied);
        assert_eq!(e["id"], "b");
        assert_eq!(e["error"]["permission"], "accessibility");
        assert_eq!(e["error"]["state"], "denied");

        let all = set(
            PermissionState::Granted,
            PermissionState::Denied,
            PermissionState::Unknown,
        );
        assert_eq!(
            all.to_json(),
            json!({"accessibility": "granted", "screen_recording": "denied", "post_event": "unknown"})
        );
    }

    #[test]
    fn names_and_constants_agree() {
        for name in PERMISSIONS {
            assert_eq!(Permission::parse(name).expect(name).as_str(), name);
        }
        for name in METHODS {
            assert!(Method::parse(name).is_some(), "{name}");
        }
    }

    #[test]
    fn capture_methods_are_registered() {
        assert_eq!(Method::parse("capture"), Some(Method::Capture));
        assert_eq!(
            Method::parse("capture_all_windows"),
            Some(Method::CaptureAllWindows)
        );
        let d = describe_result("0.1.0", false);
        assert_eq!(d["methods"], json!(METHODS));
        let listed = d["methods"].as_array().expect("array");
        assert!(listed.contains(&json!("capture")));
        assert!(listed.contains(&json!("capture_all_windows")));
    }

    #[test]
    fn parse_capture_params_accepts_a_valid_request() {
        let got = parse_capture_params(&json!({"pid": 42, "window_id": 7, "image": "/tmp/a.png"}))
            .expect("valid");
        assert_eq!(
            got,
            CaptureParams {
                pid: 42,
                window_id: 7,
                image: "/tmp/a.png".to_string()
            }
        );
    }

    #[test]
    fn parse_capture_params_rejects_each_invalid_shape() {
        let long = format!("/{}.png", "a".repeat(MAX_IMAGE_PATH_BYTES));
        let ok = |pid: Value, window_id: Value, image: Value| json!({"pid": pid, "window_id": window_id, "image": image});
        let invalid = [
            json!([]),
            json!(null),
            json!({"window_id": 7, "image": "/a.png"}),
            json!({"pid": 1, "image": "/a.png"}),
            json!({"pid": 1, "window_id": 7}),
            ok(json!(0), json!(7), json!("/a.png")),
            ok(json!(-1), json!(7), json!("/a.png")),
            ok(json!(1.5), json!(7), json!("/a.png")),
            ok(json!("42"), json!(7), json!("/a.png")),
            ok(json!(true), json!(7), json!("/a.png")),
            ok(json!(2147483648_u64), json!(7), json!("/a.png")),
            ok(json!(1), json!(0), json!("/a.png")),
            ok(json!(1), json!(4294967296_u64), json!("/a.png")),
            ok(json!(1), json!(7), json!("a.png")),
            ok(json!(1), json!(7), json!("/tmp/a.jpg")),
            ok(json!(1), json!(7), json!("/tmp/a\u{0}.png")),
            ok(json!(1), json!(7), json!(long)),
            ok(json!(1), json!(7), json!(5)),
            json!({"pid": 1, "window_id": 7, "image": "/a.png", "ax_helper": "/x"}),
        ];
        for params in invalid {
            let err = parse_capture_params(&params).expect_err(&params.to_string());
            assert_eq!(err.code, "invalid_params", "{params}");
        }
        // The longest allowed path is still accepted.
        let longest = format!("/{}.png", "a".repeat(MAX_IMAGE_PATH_BYTES - 5));
        assert_eq!(longest.len(), MAX_IMAGE_PATH_BYTES);
        assert!(parse_capture_params(&ok(json!(1), json!(7), json!(longest))).is_ok());
    }

    fn capture_report() -> Value {
        json!({
            "window_id": 7,
            "owner_pid": 42,
            "window_frame_points": {"x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0},
            "filter_content_rect_points": {"x": 0.0, "y": 0.0, "width": 2.0, "height": 2.0},
            "point_pixel_scale": 1.0,
            "image_width": 2,
            "image_height": 2,
            "capture_start_uptime_ns": 1,
            "capture_end_uptime_ns": 2,
            "capture_end_unix_seconds": 0.0,
            "image_path": "/tmp/a.png",
            "capture_type": "desktop_independent_window",
            "window_scope": "onscreen_candidates",
            "onscreen_before": true,
            "onscreen_after": true,
            "occlusion_checked": false
        })
    }

    #[test]
    fn check_capture_result_accepts_the_helper_report_and_rejects_mismatches() {
        let params = CaptureParams {
            pid: 42,
            window_id: 7,
            image: "/tmp/a.png".to_string(),
        };
        let report = capture_report();
        assert_eq!(
            check_capture_result(Method::Capture, &params, &report),
            Ok(())
        );
        let mut all = report.clone();
        all["window_scope"] = json!("all_window_candidates");
        assert_eq!(
            check_capture_result(Method::CaptureAllWindows, &params, &all),
            Ok(())
        );
        assert!(check_capture_result(Method::Capture, &params, &all).is_err());
        assert!(check_capture_result(Method::CaptureAllWindows, &params, &report).is_err());
        assert!(check_capture_result(Method::Describe, &params, &report).is_err());

        let broken: [(&str, Value); 14] = [
            ("window_id", json!(8)),
            ("owner_pid", json!(43)),
            ("image_path", json!("/tmp/b.png")),
            ("image_width", json!(0)),
            ("image_width", json!(8193)),
            ("image_width", json!("2")),
            ("image_height", json!(0)),
            ("capture_type", json!("full_screen")),
            ("occlusion_checked", json!(true)),
            ("occlusion_checked", Value::Null),
            ("onscreen_after", json!("yes")),
            (
                "window_frame_points",
                json!({"x": 0.0, "y": 0.0, "width": 0.0, "height": 2.0}),
            ),
            (
                "window_frame_points",
                json!({"y": 0.0, "width": 2.0, "height": 2.0}),
            ),
            ("window_scope", json!("onscreen")),
        ];
        for (field, value) in broken {
            let mut bad = report.clone();
            bad[field] = value.clone();
            let err = check_capture_result(Method::Capture, &params, &bad)
                .expect_err(&format!("{field} = {value}"));
            assert!(err.contains(field), "{err} must name {field}");
        }
        let mut missing = report.clone();
        missing
            .as_object_mut()
            .expect("object")
            .remove("occlusion_checked");
        assert!(check_capture_result(Method::Capture, &params, &missing).is_err());
        assert!(check_capture_result(Method::Capture, &params, &json!([])).is_err());
    }

    #[test]
    fn capture_sources_never_post_input_or_prompt_for_permissions() {
        let capture = include_str!("../ReachAgentCapture.swift");
        for needed in [
            "SCScreenshotManager",
            "desktopIndependentWindow",
            "O_EXCL",
            "O_NOFOLLOW",
        ] {
            assert!(
                capture.contains(needed),
                "ReachAgentCapture.swift must contain {needed}"
            );
        }
        for forbidden in [
            "CGEvent",
            "postToPid",
            "CGEventPost",
            "AXUIElement",
            "CGRequestScreenCaptureAccess",
            "AXIsProcessTrustedWithOptions",
            "CGPreflightScreenCaptureAccess",
            "CGWindowListCreateImage",
            "@main",
        ] {
            assert!(
                !capture.contains(forbidden),
                "ReachAgentCapture.swift must not contain {forbidden}"
            );
        }
        let service = include_str!("../ReachAgent.swift");
        for forbidden in ["ScreenCaptureKit", "SCScreenshotManager"] {
            assert!(
                !service.contains(forbidden),
                "ReachAgent.swift must not contain {forbidden}"
            );
        }
        assert_eq!(
            service.matches("SystemCaptureBackend()").count(),
            1,
            "the real capture backend is created only in main"
        );
    }

    #[test]
    fn build_scripts_compile_the_service_files() {
        for script in [
            include_str!("../build-agent.sh"),
            include_str!("../build-agent-app.sh"),
        ] {
            assert!(script.contains(
                "ReachAgent.swift ReachAgentCapture.swift ReachAgentAccessibility.swift ReachAgentInput.swift ReachAgentInputSystem.swift ReachAgentObservation.swift"
            ));
        }
    }

    #[test]
    fn accessibility_methods_are_registered() {
        assert_eq!(Method::parse("accessibility"), Some(Method::Accessibility));
        assert_eq!(
            Method::parse("accessibility_subtree"),
            Some(Method::AccessibilitySubtree)
        );
        assert_eq!(METHODS.len(), 11);
        let d = describe_result("0.1.0", true);
        assert_eq!(d["methods"], json!(METHODS));
        let listed = d["methods"].as_array().expect("array");
        assert!(listed.contains(&json!("accessibility")));
        assert!(listed.contains(&json!("accessibility_subtree")));
    }

    #[test]
    fn observation_methods_are_registered() {
        for (name, method) in [
            ("input_source", Method::InputSource),
            ("secure_input", Method::SecureInput),
            ("pointer_location", Method::PointerLocation),
            ("bundle_windows", Method::BundleWindows),
        ] {
            assert_eq!(Method::parse(name), Some(method));
            assert!(OBSERVATION_METHODS.contains(&name));
            assert!(METHODS.contains(&name));
        }
        assert!(!OBSERVATION_METHODS.contains(&"action"));
        assert!(!METHODS.contains(&"action"));
    }

    #[test]
    fn observation_params_rules() {
        assert_eq!(parse_no_observation_params(None), Ok(()));
        assert_eq!(parse_no_observation_params(Some(&json!({}))), Ok(()));
        for bad in [json!({"select": "x"}), json!([]), json!(null), json!("x")] {
            let err = parse_no_observation_params(Some(&bad)).expect_err("rejected");
            assert_eq!(err.code, "invalid_params", "{bad}");
        }
        assert_eq!(
            parse_bundle_windows_params(&json!({"bundle_id": "com.apple.Safari"})),
            Ok("com.apple.Safari".to_string())
        );
        let longest = "a".repeat(255);
        assert!(parse_bundle_windows_params(&json!({ "bundle_id": longest })).is_ok());
        for bad in [
            json!({}),
            json!({"bundle_id": ""}),
            json!({"bundle_id": "a".repeat(256)}),
            json!({"bundle_id": "bad id"}),
            json!({"bundle_id": "a/b"}),
            json!({"bundle_id": "注音"}),
            json!({"bundle_id": 1}),
            json!({"bundle_id": "a.b", "pid": 1}),
            json!([]),
        ] {
            let err = parse_bundle_windows_params(&bad).expect_err("rejected");
            assert_eq!(err.code, "invalid_params", "{bad}");
        }
    }

    fn observation_common() -> Value {
        json!({"backend": "fake", "scope": "read_only_unverified", "action_allowed": false,
               "used_by_input_gate": false})
    }

    fn with_common(fields: Value) -> Value {
        let mut value = observation_common();
        value
            .as_object_mut()
            .expect("object")
            .extend(fields.as_object().expect("object").clone());
        value
    }

    #[test]
    fn check_observation_result_accepts_the_reports_and_rejects_mismatches() {
        let source = with_common(
            json!({"input_source_id": "org.local.reach.fake-input-source",
            "input_mode_id": null, "localized_name": "Fake", "source_type": null,
            "selection_changed": false}),
        );
        let secure = with_common(json!({"secure_event_input_enabled": false}));
        let pointer = with_common(
            json!({"x": 100.5, "y": 200.25, "coordinate_space": "global_top_left_points"}),
        );
        let windows = with_common(json!({"bundle_id": "a.b", "owner_pids": [42],
            "windows": [{"window_id": 8, "owner_pid": 42, "layer": 20, "alpha": 1.0, "z_index": 1,
                         "bounds": {"x": 120.0, "y": 300.0, "width": 240.0, "height": 40.0}}],
            "truncated": false, "window_scope": "onscreen_only", "window_titles": "not_read"}));
        let cases = [
            (Method::InputSource, &source),
            (Method::SecureInput, &secure),
            (Method::PointerLocation, &pointer),
            (Method::BundleWindows, &windows),
        ];
        for (method, report) in cases {
            assert_eq!(
                check_observation_result(method, Some("a.b"), report),
                Ok(())
            );
            for (field, value) in [
                ("backend", json!("other")),
                ("scope", json!("verified")),
                ("action_allowed", json!(true)),
                ("used_by_input_gate", json!(true)),
            ] {
                let mut broken = report.clone();
                broken[field] = value;
                let err = check_observation_result(method, Some("a.b"), &broken).expect_err(field);
                assert!(err.contains(field), "{method:?} {field}: {err}");
            }
        }
        let mut system = source.clone();
        system["backend"] = json!("system");
        assert_eq!(
            check_observation_result(Method::InputSource, None, &system),
            Ok(())
        );
        for (method, report, field, value) in [
            (Method::InputSource, &source, "input_source_id", json!("")),
            (Method::InputSource, &source, "input_mode_id", json!(1)),
            (
                Method::InputSource,
                &source,
                "selection_changed",
                json!(true),
            ),
            (
                Method::SecureInput,
                &secure,
                "secure_event_input_enabled",
                json!("no"),
            ),
            (Method::PointerLocation, &pointer, "x", json!("1")),
            (
                Method::PointerLocation,
                &pointer,
                "coordinate_space",
                json!("cocoa"),
            ),
            (Method::BundleWindows, &windows, "bundle_id", json!("c.d")),
            (
                Method::BundleWindows,
                &windows,
                "window_titles",
                json!("read"),
            ),
            (
                Method::BundleWindows,
                &windows,
                "window_scope",
                json!("all"),
            ),
            (Method::BundleWindows, &windows, "owner_pids", json!([0])),
            (
                Method::BundleWindows,
                &windows,
                "owner_pids",
                json!(vec![1; 9]),
            ),
            (Method::BundleWindows, &windows, "truncated", json!(null)),
        ] {
            let mut broken = report.clone();
            broken[field] = value;
            let err = check_observation_result(method, Some("a.b"), &broken).expect_err(field);
            assert!(err.contains(field), "{method:?} {field}: {err}");
        }
        let mut too_many = windows.clone();
        too_many["windows"] = json!(vec![windows["windows"][0].clone(); 65]);
        let err = check_observation_result(Method::BundleWindows, Some("a.b"), &too_many)
            .expect_err("65 windows");
        assert!(err.contains("windows"), "{err}");
        for (field, value) in [
            ("owner_pid", json!(43)),
            ("window_id", json!(0)),
            ("layer", json!("20")),
            ("z_index", json!(-1)),
            ("alpha", json!(null)),
            ("bounds", json!({"x": 0.0, "y": 0.0, "width": 1.0})),
        ] {
            let mut broken = windows.clone();
            broken["windows"][0][field] = value;
            let err = check_observation_result(Method::BundleWindows, Some("a.b"), &broken)
                .expect_err(field);
            assert!(err.contains(field), "{field}: {err}");
        }
        assert!(check_observation_result(Method::BundleWindows, None, &windows).is_err());
        assert!(check_observation_result(Method::Capture, None, &secure).is_err());
        assert!(check_observation_result(Method::SecureInput, None, &json!([])).is_err());
    }

    /// The observation file reads state only: it never selects an input source, never moves
    /// the pointer, never posts an event, never asks for a permission and never reads a title.
    #[test]
    fn observation_source_is_read_only_and_never_prompts() {
        let source = include_str!("../ReachAgentObservation.swift");
        for needed in [
            "TISCopyCurrentKeyboardInputSource",
            "IsSecureEventInputEnabled",
            "NSEvent.mouseLocation",
            "CGWindowListCopyWindowInfo",
            "FakeObservationBackend",
            "\"observation_failed\"",
            "\"used_by_input_gate\"] = false",
            &format!("let maxObservedProcesses = {MAX_OBSERVED_PROCESSES}"),
            &format!("let maxObservedWindows = {MAX_OBSERVED_WINDOWS}"),
        ] {
            assert!(
                source.contains(needed),
                "ReachAgentObservation.swift must contain {needed}"
            );
        }
        for forbidden in [
            "TISSelectInputSource",
            "TISDeselectInputSource",
            "TISEnableInputSource",
            "TISDisableInputSource",
            "TISCreateInputSourceList",
            "TISSetInputMethodKeyboardLayoutOverride",
            "TISRegisterInputSource",
            "EnableSecureEventInput",
            "DisableSecureEventInput",
            "CGEvent",
            "CGWarpMouseCursorPosition",
            "CGDisplayMoveCursorToPoint",
            "CGAssociateMouseAndMouseCursorPosition",
            "AXIsProcessTrusted",
            "AXUIElement",
            "CGRequest",
            "CGPreflight",
            "kCGWindowName",
            "CGWindowListCreateImage",
            "SCScreenshotManager",
            "NSWorkspace",
            ".activate(",
            "NSAppleScript",
            "Process(",
            "@main",
            "exit(",
        ] {
            assert!(
                !source.contains(forbidden),
                "ReachAgentObservation.swift must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn parse_accessibility_params_rules() {
        assert_eq!(
            parse_accessibility_params(&json!({"pid": 42})).expect("valid"),
            AccessibilityParams {
                pid: 42,
                windows_only: false
            }
        );
        assert_eq!(
            parse_accessibility_params(&json!({"pid": 42, "windows_only": true})).expect("valid"),
            AccessibilityParams {
                pid: 42,
                windows_only: true
            }
        );
        for bad in [
            json!([]),
            json!(null),
            json!({}),
            json!({"pid": 0}),
            json!({"pid": -1}),
            json!({"pid": 1.5}),
            json!({"pid": "42"}),
            json!({"pid": true}),
            json!({"pid": 2147483648u64}),
            json!({"pid": 42, "windows_only": 1}),
            json!({"pid": 42, "windows_only": "true"}),
            json!({"pid": 1, "ax_helper": "/x"}),
        ] {
            let err = parse_accessibility_params(&bad).expect_err(&format!("{bad} must fail"));
            assert_eq!(err.code, "invalid_params", "{bad}");
        }
    }

    #[test]
    fn parse_subtree_params_rules() {
        assert_eq!(
            parse_subtree_params(&json!({"pid": 42, "source": "main", "path": [0, 1]}))
                .expect("valid"),
            SubtreeParams {
                pid: 42,
                source: "main".into(),
                path: vec![0, 1]
            }
        );
        for ok in [
            json!({"pid": 42, "source": "focused", "path": []}),
            json!({"pid": 42, "source": "windows", "path": [31, 159]}),
        ] {
            parse_subtree_params(&ok).unwrap_or_else(|e| panic!("{ok} must pass: {e:?}"));
        }
        for bad in [
            json!({"source": "main", "path": []}),
            json!({"pid": 42, "path": []}),
            json!({"pid": 42, "source": "main"}),
            json!({"pid": 0, "source": "main", "path": []}),
            json!({"pid": 42, "source": "other", "path": []}),
            json!({"pid": 42, "source": "Main", "path": []}),
            json!({"pid": 42, "source": 1, "path": []}),
            json!({"pid": 42, "source": "main", "path": "0"}),
            json!({"pid": 42, "source": "main", "path": [-1]}),
            json!({"pid": 42, "source": "main", "path": [160]}),
            json!({"pid": 42, "source": "main", "path": [1.5]}),
            json!({"pid": 42, "source": "main", "path": ["0"]}),
            json!({"pid": 42, "source": "main", "path": [0, 0, 0, 0, 0, 0, 0, 0, 0]}),
            json!({"pid": 42, "source": "windows", "path": []}),
            json!({"pid": 42, "source": "windows", "path": [32]}),
            json!({"pid": 42, "source": "main", "path": [], "ax_helper": "/x"}),
        ] {
            let err = parse_subtree_params(&bad).expect_err(&format!("{bad} must fail"));
            assert_eq!(err.code, "invalid_params", "{bad}");
        }
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

    #[test]
    fn check_accessibility_result_accepts_the_helper_report_and_rejects_mismatches() {
        let full = AccessibilityParams {
            pid: 42,
            windows_only: false,
        };
        let only = AccessibilityParams {
            pid: 42,
            windows_only: true,
        };
        let report = accessibility_report();
        assert_eq!(check_accessibility_result(&full, &report), Ok(()));
        assert!(check_accessibility_result(&only, &report).is_err());
        let mut windows_only = report.clone();
        windows_only["tree_scope"] = json!("windows_only");
        assert_eq!(check_accessibility_result(&only, &windows_only), Ok(()));
        assert!(check_accessibility_result(&full, &windows_only).is_err());
        for (field, value) in [
            ("schema_version", json!(2)),
            ("status", json!("process_unavailable")),
            ("pid", json!(43)),
            ("action_allowed", json!(true)),
            ("scope", json!("other")),
            ("cg_window_binding", json!("verified")),
            ("window_count_reported", json!(-1)),
            ("node_count", json!(161)),
            ("truncated", json!("no")),
            ("windows", json!({})),
        ] {
            let mut broken = report.clone();
            broken[field] = value;
            let err = check_accessibility_result(&full, &broken).expect_err(field);
            assert!(err.contains(field), "{field}: {err}");
        }
        let mut node_count_text = report.clone();
        node_count_text["node_count"] = json!("1");
        let err = check_accessibility_result(&full, &node_count_text).unwrap_err();
        assert!(err.contains("node_count"), "{err}");
        let mut too_many = report.clone();
        too_many["windows"] = json!(vec![json!({}); 33]);
        let err = check_accessibility_result(&full, &too_many).expect_err("33 windows");
        assert!(err.contains("windows"), "{err}");
        let mut no_fallback = report.clone();
        no_fallback
            .as_object_mut()
            .expect("object")
            .remove("fallback");
        let err = check_accessibility_result(&full, &no_fallback).expect_err("fallback");
        assert!(err.contains("fallback"), "{err}");
        assert!(check_accessibility_result(&full, &json!([])).is_err());
    }

    #[test]
    fn check_subtree_result_accepts_the_helper_report_and_rejects_mismatches() {
        let params = SubtreeParams {
            pid: 42,
            source: "main".into(),
            path: vec![0, 1],
        };
        let report = json!({
            "schema_version": 1, "status": "ok", "pid": 42,
            "process_start_unix_seconds": 1.0,
            "process_start_source": "kernel_sysctl_kern_proc",
            "source": "main", "path": [0, 1],
            "node": {"path": [0, 1], "owner_pid": 42, "role": "AXGroup", "title": null,
                     "actions": [], "frame": null, "children": []},
            "node_count": 1, "truncated": false,
            "scope": "read_only_subtree_unverified",
            "cg_window_binding": "unverified", "action_allowed": false
        });
        assert_eq!(check_subtree_result(&params, &report), Ok(()));
        for (field, value) in [
            ("pid", json!(43)),
            ("source", json!("focused")),
            ("path", json!([0, 2])),
            ("node", json!([])),
            ("node_count", json!(0)),
            ("scope", json!("other")),
            ("action_allowed", json!(true)),
        ] {
            let mut broken = report.clone();
            broken[field] = value;
            let err = check_subtree_result(&params, &broken).expect_err(field);
            assert!(err.contains(field), "{field}: {err}");
        }
        assert!(check_subtree_result(&params, &json!(null)).is_err());
    }

    #[test]
    fn accessibility_source_is_read_only_and_never_prompts() {
        let ax = include_str!("../ReachAgentAccessibility.swift");
        for needed in [
            "AXUIElementCopyAttributeValue",
            "AXUIElementCreateApplication",
            "kAXWindowsAttribute",
            "FakeAccessibilityBackend",
        ] {
            assert!(
                ax.contains(needed),
                "ReachAgentAccessibility.swift must contain {needed}"
            );
        }
        for forbidden in [
            "AXUIElementPerformAction",
            "AXUIElementSetAttributeValue",
            "AXIsProcessTrusted",
            "CGEvent",
            "postToPid",
            "CGEventPost",
            "CGRequestScreenCaptureAccess",
            "CGWindowListCreateImage",
            "@main",
            "exit(",
        ] {
            assert!(
                !ax.contains(forbidden),
                "ReachAgentAccessibility.swift must not contain {forbidden}"
            );
        }
        let service = include_str!("../ReachAgent.swift");
        assert_eq!(
            service.matches("SystemAccessibilityBackend()").count(),
            1,
            "the real accessibility backend is created only in main"
        );
    }

    #[test]
    fn service_doc_lists_the_accessibility_methods() {
        let doc = include_str!("../docs/AGENT-SERVICE.md");
        for needle in [
            "accessibility_subtree",
            "ax_failed",
            "windows_only",
            "尚未驗證",
        ] {
            assert!(
                doc.contains(needle),
                "docs/AGENT-SERVICE.md must mention {needle}"
            );
        }
    }

    #[test]
    fn service_doc_lists_the_capture_methods() {
        let doc = include_str!("../docs/AGENT-SERVICE.md");
        for needle in [
            "capture_all_windows",
            "capture_failed",
            "screen_recording",
            "image_sha256",
        ] {
            assert!(
                doc.contains(needle),
                "docs/AGENT-SERVICE.md must mention {needle}"
            );
        }
    }

    #[test]
    fn swift_source_declares_the_same_protocol() {
        let swift = include_str!("../ReachAgent.swift");
        assert!(swift.contains("let protocolVersion = 1"));
        let quoted = |names: &[&str]| {
            names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        for declaration in [
            format!("let methods = [{}]", quoted(&METHODS)),
            format!("let permissionNames = [{}]", quoted(&PERMISSIONS)),
            format!("let maxLineBytes = {MAX_LINE_BYTES}"),
        ] {
            assert!(
                swift.lines().any(|l| l == declaration),
                "ReachAgent.swift must declare `{declaration}`"
            );
        }
        for name in METHODS.into_iter().chain(PERMISSIONS) {
            assert!(
                swift.contains(&format!("\"{name}\"")),
                "ReachAgent.swift must name {name}"
            );
        }
        for code in [
            "invalid_json",
            "version_mismatch",
            "missing_field",
            "unknown_method",
            "invalid_params",
            "peer_uid_mismatch",
            "permission_denied",
            "not_requestable",
            "capture_failed",
            "ax_failed",
        ] {
            assert!(
                swift.contains(&format!("\"{code}\"")),
                "ReachAgent.swift must use the error code {code}"
            );
        }
        for needle in ["getpeereid", "--self-test", "--test-mode", "--serve"] {
            assert!(
                swift.contains(needle),
                "ReachAgent.swift must contain {needle}"
            );
        }
        for forbidden in [
            "CGEvent",
            "postToPid",
            "CGEventPost",
            "AXUIElementPerformAction",
            "AXUIElementCopyAttributeValue",
            "CGWindowListCreateImage",
            "SCScreenshotManager",
            "security import",
            "add-trusted-cert",
        ] {
            assert!(
                !swift.contains(forbidden),
                "ReachAgent.swift must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn build_agent_script_has_no_signing_or_keychain_commands() {
        let script = include_str!("../build-agent.sh");
        assert!(script.contains("--self-test"));
        for forbidden in ["codesign", "security ", "open "] {
            assert!(
                !script.contains(forbidden),
                "build-agent.sh must not contain {forbidden}"
            );
        }
    }

    /// What `build-agent-app.sh` must never do: certificate, keychain or TCC handling,
    /// privileged or destructive commands. Substrings first, then whole words.
    fn forbidden_in_app_script(script: &str) -> Vec<String> {
        let mut found = Vec::new();
        for needle in ["import", "add-trusted-cert", "create-keychain", "keychain"] {
            if script.contains(needle) {
                found.push(needle.to_string());
            }
        }
        for word in script.split_whitespace() {
            let word = word.trim_matches(|c: char| !(c.is_alphanumeric() || c == '-'));
            if ["security", "tccutil", "sudo", "open", "defaults", "rm"].contains(&word) {
                found.push(word.to_string());
            }
        }
        found
    }

    #[test]
    fn agent_app_plist_declares_identity_and_permissions() {
        let plist = include_str!("../ReachAgent-Info.plist");
        assert!(plist.contains("<string>dev.faberline.reach.agent</string>"));
        let flat = plist.split_whitespace().collect::<Vec<_>>().join("");
        assert!(
            flat.contains("<key>LSUIElement</key><true/>"),
            "LSUIElement must be true"
        );
        assert!(
            flat.contains("<key>CFBundleExecutable</key><string>ReachAgent</string>"),
            "CFBundleExecutable must be ReachAgent"
        );
        for key in [
            "NSAccessibilityUsageDescription",
            "NSScreenCaptureUsageDescription",
            "ReachPostEventUsageDescription",
        ] {
            assert!(
                plist.contains(&format!("<key>{key}</key>")),
                "Info.plist must have {key}"
            );
        }
    }

    #[test]
    fn agent_app_script_never_touches_certificates_keychain_or_tcc() {
        let script = include_str!("../build-agent-app.sh");
        for needed in [
            "codesign",
            "REACH_SIGN_IDENTITY",
            "--adhoc",
            "dev.faberline.reach.agent",
        ] {
            assert!(
                script.contains(needed),
                "build-agent-app.sh must contain {needed}"
            );
        }
        assert_eq!(
            forbidden_in_app_script(script),
            Vec::<String>::new(),
            "build-agent-app.sh must not handle certificates, the keychain or TCC"
        );
        // The check itself fails for the commands it is meant to catch, and not for plain words.
        assert_eq!(
            forbidden_in_app_script("security import cert.p12"),
            ["import", "security"]
        );
        assert_eq!(
            forbidden_in_app_script("sudo tccutil reset All; rm -rf x"),
            ["sudo", "tccutil", "rm"]
        );
        assert!(forbidden_in_app_script("perform the form step").is_empty());
    }

    #[cfg(unix)]
    fn run_app_script(args: &[&str], identity: Option<&str>) -> std::process::Output {
        let mut command = std::process::Command::new("sh");
        command
            .arg("build-agent-app.sh")
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env_remove("REACH_SIGN_IDENTITY");
        if let Some(identity) = identity {
            command.env("REACH_SIGN_IDENTITY", identity);
        }
        command.output().expect("sh must run")
    }

    #[cfg(unix)]
    #[test]
    fn agent_app_script_stops_without_an_identity() {
        for identity in [None, Some("")] {
            let output = run_app_script(&[], identity);
            assert_eq!(output.status.code(), Some(2), "identity {identity:?}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("REACH_SIGN_IDENTITY"),
                "stderr must explain the missing identity: {stderr}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn agent_app_script_rejects_unknown_arguments() {
        assert_eq!(run_app_script(&["--bogus"], None).status.code(), Some(2));
        assert_eq!(
            run_app_script(&["--adhoc", "--adhoc"], None).status.code(),
            Some(2)
        );
    }

    #[test]
    fn build_doc_describes_the_signing_flow() {
        let doc = include_str!("../docs/BUILD.md");
        for needle in [
            "build-agent-app.sh",
            "REACH_SIGN_IDENTITY",
            "--adhoc",
            "程式碼簽署",
            "designated requirement",
            "## 權限與簽章\n",
        ] {
            assert!(doc.contains(needle), "docs/BUILD.md must mention {needle}");
        }
    }
}
