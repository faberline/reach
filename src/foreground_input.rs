//! Offline model of the foreground, user-present input mode (plan I-1).
//!
//! Request schema, arming data, gates, `attempt`/`result` receipts and the
//! delivery sequence, all against traits. Nothing here calls an OS API, loads a
//! driver, starts a process or sends an event: the only `EventPoster` in this
//! crate is the fake used by the tests. `cu-agent` does not use this module, so
//! `action` still reports `unsupported` and no response authorizes input.
//! See `docs/plans/input-injection-plan.md`, sections 3, 4.3 and 4.4.
use crate::input_guard::{
    self, CurrentWindow, Denial, Destination, ForegroundRequest, Observation, WindowPoint,
};
use crate::Target;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

pub const ATTEMPT_SCHEMA: &str = "reach-input-attempt-v1";
pub const RESULT_SCHEMA: &str = "reach-input-result-v1";

/// The only bundle ids an arming may list (plan Q4): the local event fixture and
/// Safari. Chrome is not approved. There are no wildcards.
pub const APPROVED_BUNDLE_IDS: &[&str] = &[
    "org.local.rust-computer-use.target-event-fixture",
    "com.apple.Safari",
];
/// Longest lifetime of one arming (plan Q6: 10 minutes).
pub const MAX_ARM_TTL: Duration = Duration::from_secs(600);
/// The only macOS virtual key codes v1 may send (plan 1.3 and Q10): 18, 23, 38, 44 (the
/// Zhuyin keys), 49 (space) and 36 (Return). No modifier (54-63, including Fn and Caps Lock),
/// no function or system key. ReachAgentInput.swift has the same list; a static test checks it.
pub const APPROVED_KEYCODES: &[u16] = &[18, 23, 36, 38, 44, 49];
/// Plan Q6 asks for an event limit per arming but gives no number. This is a
/// conservative cap for the small click/key experiments of I-4 through I-6.
pub const MAX_ARM_EVENTS: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unsupported,
    Refused,
    PostedUnverified,
    DeliveryUnknown,
}

impl Status {
    pub const fn as_str(self) -> &'static str {
        match self {
            Status::Unsupported => "unsupported",
            Status::Refused => "refused",
            Status::PostedUnverified => "posted_unverified",
            Status::DeliveryUnknown => "delivery_unknown",
        }
    }

    pub const fn exit_code(self) -> u8 {
        match self {
            Status::Unsupported | Status::PostedUnverified => 3,
            Status::Refused | Status::DeliveryUnknown => 1,
        }
    }
}

/// First version: exactly these three events. Coordinates are window points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    PointerMove {
        x: f64,
        y: f64,
    },
    /// Left button only.
    Click {
        x: f64,
        y: f64,
    },
    /// No modifier keys.
    KeyTap {
        keycode: u16,
    },
}

impl Event {
    fn point(&self) -> Option<WindowPoint> {
        match *self {
            Event::PointerMove { x, y } | Event::Click { x, y } => Some(WindowPoint { x, y }),
            Event::KeyTap { .. } => None,
        }
    }

    fn to_json(self) -> Value {
        match self {
            Event::PointerMove { x, y } => json!({"kind": "pointer_move", "x": x, "y": y}),
            Event::Click { x, y } => json!({"kind": "click", "button": "left", "x": x, "y": y}),
            Event::KeyTap { keycode } => json!({"kind": "key_tap", "keycode": keycode}),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TargetSpec {
    pub bundle_id: String,
    pub pid: i32,
    pub window_id: u32,
    pub process_start_unix_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionRequest {
    pub id: String,
    pub target: TargetSpec,
    pub observation_sha256: String,
    pub event: Event,
    pub evidence_dir: String,
}

/// A request that never reached delivery. `status` is `Unsupported` or `Refused`.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    pub status: Status,
    pub gate: &'static str,
    pub reason: String,
}

fn schema(reason: impl Into<String>) -> Rejection {
    Rejection {
        status: Status::Refused,
        gate: "schema",
        reason: reason.into(),
    }
}

fn object<'a>(
    v: &'a Value,
    name: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, Rejection> {
    let o = v
        .as_object()
        .ok_or_else(|| schema(format!("{name} must be an object")))?;
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(schema(format!("{name}: unknown field {k:?}")));
    }
    Ok(o)
}

fn field<'a>(o: &'a Map<String, Value>, name: &str, key: &str) -> Result<&'a Value, Rejection> {
    o.get(key)
        .ok_or_else(|| schema(format!("{name}.{key} is required")))
}

fn text(o: &Map<String, Value>, name: &str, key: &str, max: usize) -> Result<String, Rejection> {
    match field(o, name, key)?.as_str() {
        Some(s) if !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control) => {
            Ok(s.to_string())
        }
        _ => Err(schema(format!("{name}.{key} is invalid"))),
    }
}

fn number(o: &Map<String, Value>, name: &str, key: &str) -> Result<f64, Rejection> {
    field(o, name, key)?
        .as_f64()
        .filter(|n| n.is_finite())
        .ok_or_else(|| schema(format!("{name}.{key} must be a finite number")))
}

fn point(o: &Map<String, Value>) -> Result<(f64, f64), Rejection> {
    Ok((number(o, "event", "x")?, number(o, "event", "y")?))
}

/// Strict parse of the `action` request (plan 4.3). Callers must parse bytes with
/// `strict_json::from_slice` so duplicate keys are already rejected.
pub fn parse_request(v: &Value) -> Result<ActionRequest, Rejection> {
    let top = object(
        v,
        "request",
        &[
            "id",
            "method",
            "delivery",
            "target",
            "observation",
            "event",
            "evidence_dir",
        ],
    )?;
    if top.get("method").and_then(Value::as_str) != Some("action") {
        return Err(schema("method must be \"action\""));
    }
    match top.get("delivery").and_then(Value::as_str) {
        Some("foreground") => {}
        Some("background") => {
            return Err(Rejection {
                status: Status::Unsupported,
                gate: "delivery",
                reason: "background delivery is not supported".into(),
            })
        }
        _ => return Err(schema("delivery must be \"foreground\"")),
    }
    let id = text(top, "request", "id", 128)?;
    let t = object(
        field(top, "request", "target")?,
        "target",
        &[
            "bundle_id",
            "pid",
            "window_id",
            "process_start_unix_seconds",
        ],
    )?;
    let bundle_id = text(t, "target", "bundle_id", 255)?;
    if !bundle_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(schema("target.bundle_id has invalid characters"));
    }
    let pid = field(t, "target", "pid")?
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| schema("target.pid must be positive"))?;
    let window_id = field(t, "target", "window_id")?
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| schema("target.window_id must be positive"))?;
    let process_start = number(t, "target", "process_start_unix_seconds")?;
    if process_start <= 0.0 {
        return Err(schema("target.process_start_unix_seconds must be positive"));
    }
    let o = object(
        field(top, "request", "observation")?,
        "observation",
        &["image_sha256"],
    )?;
    let observation_sha256 = text(o, "observation", "image_sha256", 64)?;
    if observation_sha256.len() != 64
        || !observation_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(schema(
            "observation.image_sha256 must be 64 lowercase hex digits",
        ));
    }
    let evidence_dir = text(top, "request", "evidence_dir", 1024)?;
    if !evidence_dir.starts_with('/') {
        return Err(schema("evidence_dir must be an absolute path"));
    }
    let e = field(top, "request", "event")?;
    let kind = e
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| schema("event.kind is required"))?;
    let event = match kind {
        "pointer_move" => {
            let o = object(e, "event", &["kind", "x", "y"])?;
            let (x, y) = point(o)?;
            Event::PointerMove { x, y }
        }
        "click" => {
            let o = object(e, "event", &["kind", "button", "x", "y"])?;
            if field(o, "event", "button")?.as_str() != Some("left") {
                return Err(schema("event.button must be \"left\""));
            }
            let (x, y) = point(o)?;
            Event::Click { x, y }
        }
        "key_tap" => {
            let o = object(e, "event", &["kind", "keycode"])?;
            let keycode = field(o, "event", "keycode")?
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| APPROVED_KEYCODES.contains(n))
                .ok_or_else(|| schema("event.keycode is not an approved key"))?;
            Event::KeyTap { keycode }
        }
        other => return Err(schema(format!("unsupported event kind {other:?}"))),
    };
    Ok(ActionRequest {
        id,
        target: TargetSpec {
            bundle_id,
            pid,
            window_id,
            process_start_unix_seconds: process_start,
        },
        observation_sha256,
        event,
        evidence_dir,
    })
}

/// Pure data for one arming: lists, expiry and event count (plan G1, G2).
#[derive(Debug, Clone, PartialEq)]
pub struct Arming {
    bundles: Vec<String>,
    keycodes: BTreeSet<u16>,
    armed_at: Instant,
    expires_at: Instant,
    remaining: u32,
}

impl Arming {
    /// `keycodes` may be empty (pointer-only). Bundle ids must be approved.
    pub fn new(
        bundles: &[&str],
        keycodes: &[u16],
        now: Instant,
        ttl: Duration,
        max_events: u32,
    ) -> Result<Self, String> {
        if bundles.is_empty() {
            return Err("allowed bundle list is empty".into());
        }
        if let Some(b) = bundles.iter().find(|b| !APPROVED_BUNDLE_IDS.contains(b)) {
            return Err(format!("bundle id {b:?} is not approved"));
        }
        if let Some(k) = keycodes.iter().find(|k| !APPROVED_KEYCODES.contains(k)) {
            return Err(format!("keycode {k} is not approved"));
        }
        if ttl.is_zero() || ttl > MAX_ARM_TTL {
            return Err("ttl must be positive and at most 10 minutes".into());
        }
        let expires_at = now
            .checked_add(ttl)
            .ok_or_else(|| "ttl overflows the clock".to_string())?;
        if max_events == 0 || max_events > MAX_ARM_EVENTS {
            return Err(format!("max_events must be 1 through {MAX_ARM_EVENTS}"));
        }
        Ok(Self {
            bundles: bundles.iter().map(|b| b.to_string()).collect(),
            keycodes: keycodes.iter().copied().collect(),
            armed_at: now,
            expires_at,
            remaining: max_events,
        })
    }

    pub fn remaining_events(&self) -> u32 {
        self.remaining
    }

    /// Expired once past the deadline, or if the clock reads before arming.
    pub fn expired(&self, now: Instant) -> bool {
        now < self.armed_at || now > self.expires_at
    }
}

/// Disabled by default. Only an explicit `armed` value can ever pass the gates.
#[derive(Debug, Default)]
pub struct InputGate {
    arming: Option<Arming>,
    halted: bool,
    /// Every request whose `attempt` was written. A request is never run twice.
    executed: Vec<ActionRequest>,
    /// Observations already used for an attempt. The next event needs a new one.
    spent_observations: BTreeSet<String>,
}

/// Facts a backend must report immediately before delivery (plan G3, G7-G9).
/// This model only consumes them; it cannot establish them.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveFacts {
    pub current_bundle_id: String,
    pub target_frontmost: bool,
    pub focus_unchanged: bool,
    pub hid_idle: bool,
    pub secure_input_off: bool,
    pub input_source_ok: bool,
    pub point_unoccluded: bool,
}

pub struct Context<'a> {
    pub latest: &'a Observation,
    pub current: &'a CurrentWindow,
    pub live: &'a LiveFacts,
    pub now: Instant,
}

/// Where `attempt` and `result` records go. A real sink must sync and never
/// overwrite; a failed `write_attempt` means nothing may be delivered.
pub trait ReceiptSink {
    fn write_attempt(&mut self, record: &Value) -> Result<(), String>;
    fn write_result(&mut self, record: &Value) -> Result<(), String>;
}

/// Why one delivery call failed. The backend must say `NotSent` only when it is
/// certain the OS never received the event; anything else is `MaybeSent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostError {
    NotSent(String),
    MaybeSent(String),
}

/// The delivery surface. Only a test fake implements it in this crate.
/// Coordinates are window points; mapping to the screen is a backend concern.
pub trait EventPoster {
    fn move_pointer(&mut self, x: f64, y: f64) -> Result<(), PostError>;
    fn button_down(&mut self, x: f64, y: f64) -> Result<(), PostError>;
    fn button_up(&mut self, x: f64, y: f64) -> Result<(), PostError>;
    fn key_down(&mut self, keycode: u16) -> Result<(), PostError>;
    fn key_up(&mut self, keycode: u16) -> Result<(), PostError>;
    /// Identity, window and focus re-check after delivery.
    fn verify_after(&mut self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub id: String,
    pub status: Status,
    pub gate: Option<&'static str>,
    pub reason: Option<String>,
    pub attempted: bool,
    pub release_attempted: bool,
    pub release_confirmed: bool,
}

impl Outcome {
    fn stopped(id: &str, status: Status, gate: &'static str, reason: &str) -> Self {
        Self {
            id: id.to_string(),
            status,
            gate: Some(gate),
            reason: Some(reason.to_string()),
            attempted: false,
            release_attempted: false,
            release_confirmed: false,
        }
    }

    fn result_json(&self) -> Value {
        json!({"gate": self.gate, "reason": self.reason, "attempted": self.attempted,
            "release_attempted": self.release_attempted,
            "release_confirmed": self.release_confirmed})
    }

    /// Reply body. `action_allowed` is always false: no reply authorizes a next action.
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "status": self.status.as_str(), "action_allowed": false,
            "result": self.result_json()})
    }

    /// The `result` receipt written after delivery.
    fn record(&self) -> Value {
        let mut v = self.result_json();
        v["schema"] = json!(RESULT_SCHEMA);
        v["id"] = json!(self.id);
        v["status"] = json!(self.status.as_str());
        v
    }
}

type Stop = (&'static str, &'static str);

impl InputGate {
    pub fn armed(arming: Arming) -> Self {
        Self {
            arming: Some(arming),
            ..Self::default()
        }
    }

    pub fn remaining_events(&self) -> Option<u32> {
        self.arming.as_ref().map(Arming::remaining_events)
    }

    fn check(&self, req: &ActionRequest, ctx: &Context) -> Result<(), Stop> {
        let arming = self.arming.as_ref().ok_or(("G1", "input_disabled"))?;
        if self.halted {
            return Err(("G13", "halted_after_delivery_unknown"));
        }
        if arming.expired(ctx.now) {
            return Err(("G1", "arming_expired"));
        }
        if arming.remaining == 0 {
            return Err(("G1", "event_limit_reached"));
        }
        if let Some(done) = self.executed.iter().find(|r| r.id == req.id) {
            return Err(if done == req {
                ("G13", "request_already_executed")
            } else {
                ("G13", "request_id_reused")
            });
        }
        if self.spent_observations.contains(&req.observation_sha256) {
            return Err(("G5", "observation_already_used"));
        }
        if !arming.bundles.contains(&req.target.bundle_id) {
            return Err(("G2", "bundle_not_allowed"));
        }
        if let Event::KeyTap { keycode } = req.event {
            if !arming.keycodes.contains(&keycode) {
                return Err(("G2", "keycode_not_allowed"));
            }
        }
        if ctx.live.current_bundle_id != req.target.bundle_id {
            return Err(("G3", "bundle_mismatch"));
        }
        if req.target.process_start_unix_seconds != ctx.current.process_start_unix_seconds {
            return Err(("G3", "process_start_mismatch"));
        }
        if req.observation_sha256 != ctx.latest.image_sha256() {
            return Err(("G5", "observation_mismatch"));
        }
        let target = Target::new(req.target.pid, req.target.window_id)
            .map_err(|_| ("G3", "target_mismatch"))?;
        let fg = ForegroundRequest {
            destination: Destination::Window(target),
            point: req.event.point(),
            observation: ctx.latest.token(),
        };
        input_guard::review_foreground(&fg, ctx.latest, ctx.current, ctx.now).map_err(
            |d| match d {
                Denial::StaleObservation => ("G5", "observation_mismatch"),
                Denial::ObservationExpired => ("G5", "observation_expired"),
                Denial::PointOutsideWindow => ("G6", "point_outside_window"),
                Denial::TargetMismatch => ("G3", "target_mismatch"),
                Denial::CurrentIdentityMismatch => ("G3", "identity_mismatch"),
                Denial::InvalidObservation => ("G3", "invalid_observation"),
                Denial::DesktopUnsupported | Denial::ForegroundUnsupported => {
                    ("G3", "destination_unsupported")
                }
            },
        )?;
        let l = ctx.live;
        if req.event.point().is_some() && !l.point_unoccluded {
            return Err(("G7", "point_occluded"));
        }
        if !l.target_frontmost {
            return Err(("G8", "target_not_frontmost"));
        }
        if !l.focus_unchanged {
            return Err(("G8", "focus_changed"));
        }
        if !l.hid_idle {
            return Err(("G9", "hid_not_idle"));
        }
        if !l.secure_input_off {
            return Err(("G9", "secure_input_enabled"));
        }
        if !l.input_source_ok {
            return Err(("G9", "input_source_unexpected"));
        }
        Ok(())
    }

    /// Run one request: gates, `attempt` receipt, one delivery sequence, post-check,
    /// `result` receipt. Never retries. After `delivery_unknown` the gate halts and
    /// refuses every later request; a new arming replaces it.
    pub fn submit(
        &mut self,
        req: &ActionRequest,
        ctx: &Context,
        sink: &mut dyn ReceiptSink,
        poster: &mut dyn EventPoster,
    ) -> Outcome {
        if let Err((gate, reason)) = self.check(req, ctx) {
            let status = if self.arming.is_none() {
                Status::Unsupported
            } else {
                Status::Refused
            };
            return Outcome::stopped(&req.id, status, gate, reason);
        }
        let remaining = self.remaining_events().unwrap_or(0);
        let attempt = json!({
            "schema": ATTEMPT_SCHEMA, "id": req.id, "delivery": "foreground",
            "event": req.event.to_json(),
            "target": {"bundle_id": req.target.bundle_id, "pid": req.target.pid,
                "window_id": req.target.window_id,
                "process_start_unix_seconds": req.target.process_start_unix_seconds},
            "observation_sha256": req.observation_sha256,
            "evidence_dir": req.evidence_dir,
            "events_remaining_before": remaining,
        });
        if sink.write_attempt(&attempt).is_err() {
            return Outcome::stopped(&req.id, Status::Refused, "G12", "attempt_not_recorded");
        }
        if let Some(a) = self.arming.as_mut() {
            a.remaining -= 1;
        }
        // From here the request and its observation are spent, whatever happens.
        self.executed.push(req.clone());
        self.spent_observations
            .insert(req.observation_sha256.clone());
        let mut out = Outcome {
            id: req.id.clone(),
            status: Status::PostedUnverified,
            gate: None,
            reason: None,
            attempted: true,
            release_attempted: false,
            release_confirmed: false,
        };
        deliver(req.event, poster, &mut out);
        if out.status == Status::PostedUnverified && poster.verify_after().is_err() {
            mark_unknown(&mut out, "G3", "post_check_failed");
        }
        if sink.write_result(&out.record()).is_err() {
            mark_unknown(&mut out, "G12", "result_not_recorded");
        }
        if out.status == Status::DeliveryUnknown {
            self.halted = true;
        }
        out
    }
}

fn mark_unknown(out: &mut Outcome, gate: &'static str, reason: &str) {
    out.status = Status::DeliveryUnknown;
    out.gate = Some(gate);
    out.reason = Some(reason.to_string());
}

/// The backend certainly sent nothing: no event reached the OS, so the request
/// ends as `refused` (the attempt receipt still exists and the count is spent).
fn mark_not_sent(out: &mut Outcome) {
    out.status = Status::Refused;
    out.gate = Some("G10");
    out.reason = Some("press_not_sent".to_string());
}

/// One synchronous press/release. A failure after a press that may have been
/// sent attempts one release (cleanup, not a retry of the event) and ends as
/// `delivery_unknown`. A press that certainly was not sent gets no release,
/// since an unpaired release would itself be a stray event.
fn deliver(event: Event, poster: &mut dyn EventPoster, out: &mut Outcome) {
    let pressed = match event {
        Event::PointerMove { x, y } => poster.move_pointer(x, y),
        Event::Click { x, y } => poster.button_down(x, y),
        Event::KeyTap { keycode } => poster.key_down(keycode),
    };
    if let Err(PostError::NotSent(_)) = pressed {
        mark_not_sent(out);
        return;
    }
    if matches!(event, Event::PointerMove { .. }) {
        if pressed.is_err() {
            mark_unknown(out, "G10", "post_failed");
        }
        return;
    }
    let released = match event {
        Event::Click { x, y } => poster.button_up(x, y),
        Event::KeyTap { keycode } => poster.key_up(keycode),
        Event::PointerMove { .. } => return,
    };
    out.release_attempted = true;
    out.release_confirmed = released.is_ok();
    if pressed.is_err() {
        mark_unknown(out, "G10", "post_failed");
    } else if released.is_err() {
        mark_unknown(out, "G10", "release_failed");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveredStatus {
    /// No `attempt` and no `result`: nothing was delivered.
    NotAttempted,
    Reported(Status),
}

/// Read receipts back (plan G12): an `attempt` without a matching, well-formed
/// `result` means the event may have been delivered, so it is `delivery_unknown`.
pub fn recover_status(attempt: Option<&Value>, result: Option<&Value>) -> RecoveredStatus {
    let unknown = RecoveredStatus::Reported(Status::DeliveryUnknown);
    let (Some(attempt), Some(result)) = (attempt, result) else {
        return if attempt.is_none() && result.is_none() {
            RecoveredStatus::NotAttempted
        } else {
            unknown
        };
    };
    if attempt["schema"] != ATTEMPT_SCHEMA
        || result["schema"] != RESULT_SCHEMA
        || result["id"] != attempt["id"]
    {
        return unknown;
    }
    match result["status"].as_str() {
        Some("posted_unverified") => RecoveredStatus::Reported(Status::PostedUnverified),
        _ => unknown,
    }
}

#[cfg(test)]
mod tests;
