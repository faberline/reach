use super::*;
use crate::{
    input_guard::{CurrentWindow, Observation},
    Rect, Target, Window,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const SAFARI: &str = "com.apple.Safari";
const FIXTURE: &str = "org.local.rust-computer-use.target-event-fixture";

fn sha() -> String {
    "a".repeat(64)
}

fn window() -> Window {
    Window {
        target: Target::new(42, 7).unwrap(),
        layer: Some(0),
        title: None,
        bounds: Rect {
            x: 100.0,
            y: 50.0,
            width: 800.0,
            height: 600.0,
        },
    }
}

fn observation(at: Instant) -> Observation {
    Observation::new(window(), 123.5, sha(), at, 1).unwrap()
}

fn current() -> CurrentWindow {
    CurrentWindow {
        window: window(),
        process_start_unix_seconds: 123.5,
    }
}

fn request_json(event: Value) -> Value {
    json!({
        "id": "r1", "method": "action", "delivery": "foreground",
        "target": {"bundle_id": SAFARI, "pid": 42, "window_id": 7,
                   "process_start_unix_seconds": 123.5},
        "observation": {"image_sha256": sha()},
        "event": event,
        "evidence_dir": "/tmp/reach-evidence/new"
    })
}

fn click_json() -> Value {
    request_json(json!({"kind": "click", "button": "left", "x": 10.0, "y": 80.0}))
}

fn parse(v: &Value) -> ActionRequest {
    parse_request(v).unwrap()
}

fn arming(now: Instant) -> Arming {
    Arming::new(&[SAFARI], &[36, 49], now, Duration::from_secs(600), 3).unwrap()
}

fn gate_armed(now: Instant) -> InputGate {
    InputGate::armed(arming(now))
}

fn live() -> LiveFacts {
    LiveFacts {
        current_bundle_id: SAFARI.into(),
        target_frontmost: true,
        focus_unchanged: true,
        hid_idle: true,
        modifiers_released: true,
        caps_lock_off: true,
        mouse_buttons_released: true,
        secure_input_off: true,
        input_source_ok: true,
        point_unoccluded: true,
    }
}

#[derive(Default)]
struct Sink {
    attempts: Vec<Value>,
    results: Vec<Value>,
    fail_attempt: bool,
    fail_result: bool,
}
impl ReceiptSink for Sink {
    fn write_attempt(&mut self, record: &Value) -> Result<(), String> {
        if self.fail_attempt {
            return Err("disk full".into());
        }
        self.attempts.push(record.clone());
        Ok(())
    }
    fn write_result(&mut self, record: &Value) -> Result<(), String> {
        if self.fail_result {
            return Err("disk full".into());
        }
        self.results.push(record.clone());
        Ok(())
    }
}

#[derive(Default)]
struct Fake {
    calls: Vec<String>,
    fail_on: Option<&'static str>,
    fail_verify: bool,
    /// The failing call certainly sent nothing (`PostError::NotSent`).
    fail_not_sent: bool,
}
impl Fake {
    fn step(&mut self, name: &'static str, detail: String) -> Result<(), PostError> {
        self.calls.push(format!("{name}{detail}"));
        if self.fail_on == Some(name) {
            let why = format!("{name} failed");
            Err(if self.fail_not_sent {
                PostError::NotSent(why)
            } else {
                PostError::MaybeSent(why)
            })
        } else {
            Ok(())
        }
    }
}
impl EventPoster for Fake {
    fn move_pointer(&mut self, x: f64, y: f64) -> Result<(), PostError> {
        self.step("move", format!("({x},{y})"))
    }
    fn button_down(&mut self, x: f64, y: f64) -> Result<(), PostError> {
        self.step("down", format!("({x},{y})"))
    }
    fn button_up(&mut self, x: f64, y: f64) -> Result<(), PostError> {
        self.step("up", format!("({x},{y})"))
    }
    fn key_down(&mut self, keycode: u16) -> Result<(), PostError> {
        self.step("keydown", format!("({keycode})"))
    }
    fn key_up(&mut self, keycode: u16) -> Result<(), PostError> {
        self.step("keyup", format!("({keycode})"))
    }
    fn verify_after(&mut self) -> Result<(), String> {
        if self.fail_verify {
            Err("identity changed".into())
        } else {
            Ok(())
        }
    }
}

fn submit_with(
    gate: &mut InputGate,
    req: &Value,
    now: Instant,
    live: &LiveFacts,
    sink: &mut Sink,
    fake: &mut Fake,
) -> Outcome {
    submit_latest(gate, req, now, &sha(), live, sink, fake)
}

/// Like `submit_with`, but the latest observation has the given hash.
fn submit_latest(
    gate: &mut InputGate,
    req: &Value,
    now: Instant,
    latest_sha: &str,
    live: &LiveFacts,
    sink: &mut Sink,
    fake: &mut Fake,
) -> Outcome {
    let obs = Observation::new(window(), 123.5, latest_sha.to_string(), now, 1).unwrap();
    let cur = current();
    let ctx = Context {
        latest: &obs,
        current: &cur,
        live,
        now,
    };
    gate.submit(&parse(req), &ctx, sink, fake)
}

/// A request that carries a fresh observation hash `c` repeated 64 times.
fn with_obs(mut v: Value, c: char) -> (Value, String) {
    let h = c.to_string().repeat(64);
    v["observation"]["image_sha256"] = json!(h);
    (v, h)
}

fn submit(gate: &mut InputGate, req: &Value, now: Instant) -> (Outcome, Sink, Fake) {
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let out = submit_with(gate, req, now, &live(), &mut sink, &mut fake);
    (out, sink, fake)
}

fn refused(out: &Outcome, gate: &str, reason: &str) {
    assert_eq!(out.status, Status::Refused, "{out:?}");
    assert_eq!(out.gate, Some(gate), "{out:?}");
    assert_eq!(out.reason.as_deref(), Some(reason), "{out:?}");
    assert!(!out.attempted);
}

// ---- schema ----

#[test]
fn parses_the_three_event_kinds() {
    let r = parse(&request_json(
        json!({"kind": "pointer_move", "x": 1.5, "y": 2.0}),
    ));
    assert_eq!(r.event, Event::PointerMove { x: 1.5, y: 2.0 });
    assert_eq!(
        parse(&click_json()).event,
        Event::Click { x: 10.0, y: 80.0 }
    );
    let r = parse(&request_json(json!({"kind": "key_tap", "keycode": 36})));
    assert_eq!(r.event, Event::KeyTap { keycode: 36 });
    assert_eq!(r.target.bundle_id, SAFARI);
    assert_eq!(r.id, "r1");
}

#[test]
fn rejects_unknown_fields_at_every_level() {
    for path in ["top", "target", "observation", "event"] {
        let mut v = click_json();
        let obj = match path {
            "top" => v.as_object_mut().unwrap(),
            other => v[other].as_object_mut().unwrap(),
        };
        obj.insert("extra".into(), json!(1));
        let e = parse_request(&v).unwrap_err();
        assert_eq!((e.status, e.gate), (Status::Refused, "schema"), "{path}");
    }
}

#[test]
fn rejects_modifiers_drag_scroll_and_other_kinds() {
    for event in [
        json!({"kind": "key_tap", "keycode": 36, "modifiers": ["cmd"]}),
        json!({"kind": "click", "button": "left", "x": 1.0, "y": 1.0, "shift": true}),
        json!({"kind": "drag", "x": 1.0, "y": 1.0}),
        json!({"kind": "scroll", "x": 1.0, "y": 1.0}),
        json!({"kind": "type_text", "text": "a"}),
        json!({"kind": "activate_app"}),
        json!({"kind": "pointer_click", "x": 1.0, "y": 1.0}),
    ] {
        let e = parse_request(&request_json(event)).unwrap_err();
        assert_eq!((e.status, e.gate), (Status::Refused, "schema"));
    }
}

#[test]
fn rejects_non_left_button_and_bad_numbers() {
    for event in [
        json!({"kind": "click", "button": "right", "x": 1.0, "y": 1.0}),
        json!({"kind": "click", "x": 1.0, "y": 1.0}),
        json!({"kind": "click", "button": "left", "x": "1", "y": 1.0}),
        json!({"kind": "key_tap", "keycode": -1}),
        json!({"kind": "key_tap", "keycode": 128}),
        json!({"kind": "key_tap", "keycode": 55}),
        json!({"kind": "key_tap", "keycode": 57}),
        json!({"kind": "key_tap", "keycode": 63}),
        json!({"kind": "key_tap", "keycode": 103}),
        json!({"kind": "key_tap", "keycode": 0}),
        json!({"kind": "key_tap", "keycode": 1.5}),
        json!({"kind": "key_tap"}),
    ] {
        assert!(parse_request(&request_json(event)).is_err());
    }
}

#[test]
fn background_or_missing_delivery_never_reaches_the_gate() {
    let mut v = click_json();
    v["delivery"] = json!("background");
    let e = parse_request(&v).unwrap_err();
    assert_eq!((e.status, e.gate), (Status::Unsupported, "delivery"));
    for bad in [json!("desktop"), json!(null), json!(1)] {
        v["delivery"] = bad;
        assert!(parse_request(&v).is_err());
    }
    v.as_object_mut().unwrap().remove("delivery");
    assert!(parse_request(&v).is_err());
}

#[test]
fn rejects_bad_envelope_values() {
    type Mutation = Box<dyn Fn(&mut Value)>;
    let mutations: Vec<Mutation> = vec![
        Box::new(|v| v["method"] = json!("describe")),
        Box::new(|v| v["id"] = json!("")),
        Box::new(|v| v["target"]["pid"] = json!(0)),
        Box::new(|v| v["target"]["window_id"] = json!(0)),
        Box::new(|v| v["target"]["bundle_id"] = json!("")),
        Box::new(|v| v["target"]["bundle_id"] = json!("com.apple.*")),
        Box::new(|v| v["target"]["process_start_unix_seconds"] = json!(-1.0)),
        Box::new(|v| v["observation"]["image_sha256"] = json!("ABC")),
        Box::new(|v| v["evidence_dir"] = json!("relative/dir")),
        Box::new(|v| v["event"] = json!("click")),
    ];
    for m in mutations {
        let mut v = click_json();
        m(&mut v);
        assert!(parse_request(&v).is_err(), "{v}");
    }
}

#[test]
fn statuses_have_stable_names_and_exit_codes() {
    let all = [
        (Status::Unsupported, "unsupported", 3),
        (Status::Refused, "refused", 1),
        (Status::PostedUnverified, "posted_unverified", 3),
        (Status::DeliveryUnknown, "delivery_unknown", 1),
    ];
    for (s, name, code) in all {
        assert_eq!((s.as_str(), s.exit_code()), (name, code));
    }
}

// ---- arming data model ----

#[test]
fn arming_accepts_only_approved_bundles_and_bounded_values() {
    let now = Instant::now();
    let ttl = Duration::from_secs(60);
    assert!(Arming::new(&[SAFARI, FIXTURE], &[36], now, ttl, 1).is_ok());
    assert!(Arming::new(&[], &[36], now, ttl, 1).is_err());
    assert!(Arming::new(&["com.google.Chrome"], &[36], now, ttl, 1).is_err());
    assert!(Arming::new(&["*"], &[36], now, ttl, 1).is_err());
    assert!(Arming::new(&[SAFARI], &[128], now, ttl, 1).is_err());
    // Only the six plan keys (plan 1.3, Q10): no modifier, Fn, Caps Lock or system key.
    for k in APPROVED_KEYCODES {
        assert!(Arming::new(&[SAFARI], &[*k], now, ttl, 1).is_ok(), "{k}");
    }
    for k in [
        0, 3, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 103, 122, 127,
    ] {
        assert!(Arming::new(&[SAFARI], &[k], now, ttl, 1).is_err(), "{k}");
    }
    assert!(Arming::new(&[SAFARI], &[36], now, Duration::ZERO, 1).is_err());
    assert!(Arming::new(&[SAFARI], &[36], now, MAX_ARM_TTL, 1).is_ok());
    assert!(Arming::new(
        &[SAFARI],
        &[36],
        now,
        MAX_ARM_TTL + Duration::from_nanos(1),
        1
    )
    .is_err());
    assert!(Arming::new(&[SAFARI], &[36], now, ttl, 0).is_err());
    // An empty key list is allowed: pointer-only arming.
    assert!(Arming::new(&[SAFARI], &[], now, ttl, 1).is_ok());
}

#[test]
fn arming_reports_expiry_and_remaining_events() {
    let now = Instant::now();
    let a = arming(now);
    assert_eq!(a.remaining_events(), 3);
    assert!(!a.expired(now));
    assert!(!a.expired(now + Duration::from_secs(600)));
    assert!(a.expired(now + Duration::from_secs(601)));
    assert!(a.expired(now - Duration::from_nanos(1)));
}

// ---- gates ----

#[test]
fn default_gate_is_disabled_and_reports_unsupported() {
    let now = Instant::now();
    let mut gate = InputGate::default();
    let (out, sink, fake) = submit(&mut gate, &click_json(), now);
    assert_eq!(out.status, Status::Unsupported);
    assert_eq!(out.gate, Some("G1"));
    assert!(!out.attempted);
    assert!(sink.attempts.is_empty() && sink.results.is_empty());
    assert!(fake.calls.is_empty());
}

#[test]
fn g1_expired_or_exhausted_arming_refuses() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let late = now + Duration::from_secs(601);
    let out = submit_with(
        &mut gate,
        &click_json(),
        late,
        &live(),
        &mut sink,
        &mut fake,
    );
    refused(&out, "G1", "arming_expired");
    assert!(fake.calls.is_empty() && sink.attempts.is_empty());

    let mut gate = gate_armed(now);
    for (i, c) in ['1', '2', '3'].into_iter().enumerate() {
        let (v, h) = with_obs(click_json(), c);
        v_id(&mut gate, v, &h, now, i, Status::PostedUnverified);
    }
    let (v, h) = with_obs(click_json(), '4');
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let mut v = v;
    v["id"] = json!("r4");
    let out = submit_latest(&mut gate, &v, now, &h, &live(), &mut sink, &mut fake);
    refused(&out, "G1", "event_limit_reached");
    assert!(fake.calls.is_empty());
}

/// Submit `v` under the unique id `r{i}` with matching latest observation `h`.
fn v_id(gate: &mut InputGate, mut v: Value, h: &str, now: Instant, i: usize, want: Status) {
    v["id"] = json!(format!("r{i}"));
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let out = submit_latest(gate, &v, now, h, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, want, "{out:?}");
}

// ---- replay (review: the same request must never be delivered twice) ----

#[test]
fn replaying_the_same_request_is_refused_and_delivers_nothing() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (first, sink1, fake1) = submit(&mut gate, &click_json(), now);
    assert_eq!(first.status, Status::PostedUnverified);
    assert_eq!(fake1.calls, ["down(10,80)", "up(10,80)"]);
    assert_eq!(sink1.attempts.len(), 1);

    let (second, sink2, fake2) = submit(&mut gate, &click_json(), now);
    refused(&second, "G13", "request_already_executed");
    assert!(fake2.calls.is_empty() && sink2.attempts.is_empty());
    assert_eq!(gate.remaining_events(), Some(2), "a refusal costs nothing");
}

#[test]
fn the_same_id_with_different_content_is_refused() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (first, _, _) = submit(&mut gate, &click_json(), now);
    assert_eq!(first.status, Status::PostedUnverified);

    // Same id "r1", a fresh observation, but another event.
    let (v, h) = with_obs(
        request_json(json!({"kind": "click", "button": "left", "x": 11.0, "y": 80.0})),
        'b',
    );
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let out = submit_latest(&mut gate, &v, now, &h, &live(), &mut sink, &mut fake);
    refused(&out, "G13", "request_id_reused");
    assert!(fake.calls.is_empty() && sink.attempts.is_empty());
}

#[test]
fn an_old_observation_cannot_authorize_the_next_event() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (first, _, _) = submit(&mut gate, &click_json(), now);
    assert_eq!(first.status, Status::PostedUnverified);

    // New id, same observation hash (still the latest one the backend holds).
    let mut v = click_json();
    v["id"] = json!("r2");
    let (out, sink, fake) = submit(&mut gate, &v, now);
    refused(&out, "G5", "observation_already_used");
    assert!(fake.calls.is_empty() && sink.attempts.is_empty());

    // A new observation and a new id go through.
    let (mut v, h) = with_obs(click_json(), 'c');
    v["id"] = json!("r3");
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let out = submit_latest(&mut gate, &v, now, &h, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::PostedUnverified);
    assert_eq!(fake.calls, ["down(10,80)", "up(10,80)"]);
}

#[test]
fn a_request_that_failed_to_record_its_attempt_is_not_spent() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    sink.fail_attempt = true;
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    refused(&out, "G12", "attempt_not_recorded");
    // Nothing was delivered, so the same request may be submitted once more.
    let (out, _, fake) = submit(&mut gate, &click_json(), now);
    assert_eq!(out.status, Status::PostedUnverified);
    assert_eq!(fake.calls.len(), 2);
}

#[test]
fn a_failed_delivery_also_spends_the_request_and_observation() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("up");
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    // The gate halts; the halt is reported before the replay checks.
    let (out, _, fake) = submit(&mut gate, &click_json(), now);
    refused(&out, "G13", "halted_after_delivery_unknown");
    assert!(fake.calls.is_empty());
}

// ---- review items: down certainly not sent, event cap ----

#[test]
fn a_press_that_was_certainly_not_sent_gets_no_release() {
    let now = Instant::now();
    for (event, down) in [
        (
            json!({"kind": "click", "button": "left", "x": 10.0, "y": 80.0}),
            "down",
        ),
        (json!({"kind": "key_tap", "keycode": 36}), "keydown"),
    ] {
        let mut gate = gate_armed(now);
        let (mut sink, mut fake) = (Sink::default(), Fake::default());
        fake.fail_on = Some(down);
        fake.fail_not_sent = true;
        let out = submit_with(
            &mut gate,
            &request_json(event),
            now,
            &live(),
            &mut sink,
            &mut fake,
        );
        assert_eq!(out.status, Status::Refused, "{out:?}");
        assert_eq!(out.gate, Some("G10"));
        assert_eq!(out.reason.as_deref(), Some("press_not_sent"));
        assert!(out.attempted && !out.release_attempted);
        assert_eq!(
            fake.calls.len(),
            1,
            "no up/keyup after a press that never left"
        );
        assert_eq!(sink.results[0]["status"], "refused");
        // Not unknown: the gate is not halted, and the spent request stays spent.
        let (v, h) = with_obs(click_json(), 'd');
        let mut v = v;
        v["id"] = json!("r9");
        let (mut sink, mut fake) = (Sink::default(), Fake::default());
        let out = submit_latest(&mut gate, &v, now, &h, &live(), &mut sink, &mut fake);
        assert_eq!(out.status, Status::PostedUnverified);
    }
    // Pointer moves have no release either way.
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("move");
    fake.fail_not_sent = true;
    let v = request_json(json!({"kind": "pointer_move", "x": 1.0, "y": 61.0}));
    let out = submit_with(&mut gate, &v, now, &live(), &mut sink, &mut fake);
    assert_eq!(out.reason.as_deref(), Some("press_not_sent"));
}

#[test]
fn arming_caps_the_event_count() {
    let now = Instant::now();
    let ttl = Duration::from_secs(600);
    assert!(Arming::new(&[SAFARI], &[36], now, ttl, MAX_ARM_EVENTS).is_ok());
    assert!(Arming::new(&[SAFARI], &[36], now, ttl, MAX_ARM_EVENTS + 1).is_err());
    assert!(Arming::new(&[SAFARI], &[36], now, ttl, u32::MAX).is_err());
    assert_eq!(MAX_ARM_EVENTS, 20);
}

#[test]
fn g2_bundle_and_keycode_must_be_on_the_lists() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let mut v = click_json();
    v["target"]["bundle_id"] = json!("com.google.Chrome");
    let (out, _, fake) = submit(&mut gate, &v, now);
    refused(&out, "G2", "bundle_not_allowed");
    assert!(fake.calls.is_empty());

    // 18 is an approved key but not in this arming ([36, 49]).
    let v = request_json(json!({"kind": "key_tap", "keycode": 18}));
    let (out, _, fake) = submit(&mut gate, &v, now);
    refused(&out, "G2", "keycode_not_allowed");
    assert!(fake.calls.is_empty());
}

#[test]
fn g3_identity_must_match_the_live_target() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let mut other = live();
    other.current_bundle_id = FIXTURE.into();
    let out = submit_with(&mut gate, &click_json(), now, &other, &mut sink, &mut fake);
    refused(&out, "G3", "bundle_mismatch");

    let mut v = click_json();
    v["target"]["process_start_unix_seconds"] = json!(999.0);
    let (out, _, _) = submit(&mut gate, &v, now);
    refused(&out, "G3", "process_start_mismatch");

    let mut v = click_json();
    v["target"]["pid"] = json!(43);
    let (out, _, _) = submit(&mut gate, &v, now);
    refused(&out, "G3", "target_mismatch");
    assert!(fake.calls.is_empty());
}

#[test]
fn g5_observation_must_be_the_latest_and_fresh() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let mut v = click_json();
    v["observation"]["image_sha256"] = json!("b".repeat(64));
    let (out, _, _) = submit(&mut gate, &v, now);
    refused(&out, "G5", "observation_mismatch");

    // Arm late enough that the arming outlives the observation.
    let mut gate =
        InputGate::armed(Arming::new(&[SAFARI], &[36], now, Duration::from_secs(600), 3).unwrap());
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let obs = observation(now);
    let cur = current();
    let l = live();
    let ctx = Context {
        latest: &obs,
        current: &cur,
        live: &l,
        now: now + crate::input_guard::MAX_OBSERVATION_AGE + Duration::from_nanos(1),
    };
    let out = gate.submit(&parse(&click_json()), &ctx, &mut sink, &mut fake);
    refused(&out, "G5", "observation_expired");
}

#[test]
fn g6_point_must_be_inside_the_window_but_keys_have_no_point() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    for (x, y) in [(-1.0, 60.0), (800.0, 60.0), (5.0, 600.0)] {
        let v = request_json(json!({"kind": "pointer_move", "x": x, "y": y}));
        let (out, _, fake) = submit(&mut gate, &v, now);
        refused(&out, "G6", "point_outside_window");
        assert!(fake.calls.is_empty());
    }
    let v = request_json(json!({"kind": "key_tap", "keycode": 36}));
    let (out, _, _) = submit(&mut gate, &v, now);
    assert_eq!(out.status, Status::PostedUnverified);
}

#[test]
fn g6_refuses_the_whole_title_bar_band() {
    // Plan review R1: the content rect is not known offline, so the top band of the window
    // (title bar, close/minimize/zoom buttons, unified toolbar) is refused as a whole.
    assert_eq!(TITLE_BAR_BAND_POINTS, 52.0);
    let now = Instant::now();
    let mut gate = gate_armed(now);
    for (x, y) in [(10.0, 0.0), (70.0, 14.0), (10.0, 51.9), (400.0, 30.0)] {
        for v in [
            request_json(json!({"kind": "pointer_move", "x": x, "y": y})),
            request_json(json!({"kind": "click", "button": "left", "x": x, "y": y})),
        ] {
            let (out, _, fake) = submit(&mut gate, &v, now);
            refused(&out, "G6", "point_in_title_bar");
            assert!(fake.calls.is_empty());
        }
    }
    // Outside the window stays the outside reason, even above the band.
    let v = request_json(json!({"kind": "pointer_move", "x": 10.0, "y": -1.0}));
    let (out, _, _) = submit(&mut gate, &v, now);
    refused(&out, "G6", "point_outside_window");
    // The first point below the band is allowed.
    let v = request_json(json!({"kind": "pointer_move", "x": 10.0, "y": 52.0}));
    let (out, _, fake) = submit(&mut gate, &v, now);
    assert_eq!(out.status, Status::PostedUnverified);
    assert_eq!(fake.calls, ["move(10,52)"]);
}

#[test]
fn g9_held_input_decisions() {
    // Plan review R2: each modifier bit refuses; other bits do not.
    assert!(modifiers_released(0));
    for bit in [
        0x0002_0000u64,
        0x0004_0000,
        0x0008_0000,
        0x0010_0000,
        0x0080_0000,
    ] {
        assert!(!modifiers_released(bit), "{bit:#x}");
        assert!(!modifiers_released(bit | 0x100), "{bit:#x}");
    }
    // Numeric pad and the non-coalesced bit are not held modifiers.
    assert!(modifiers_released(0x0020_0000 | 0x100));
    // P31-3: Caps Lock is its own reason, alone or with other bits.
    assert!(caps_lock_off(0) && caps_lock_off(0x0002_0000 | 0x100));
    assert!(!caps_lock_off(CAPS_LOCK_FLAG) && !caps_lock_off(CAPS_LOCK_FLAG | 0x0002_0000));
    assert!(modifiers_released(CAPS_LOCK_FLAG));
    assert!(!modifiers_released(CAPS_LOCK_FLAG | 0x0002_0000));
    assert!(mouse_buttons_released(&[]));
    assert!(mouse_buttons_released(&[false, false, false]));
    assert!(!mouse_buttons_released(&[true, false, false]));
    assert!(!mouse_buttons_released(&[false, false, true]));
}

#[test]
fn g7_to_g9_live_conditions_each_refuse() {
    let now = Instant::now();
    type Mutation = fn(&mut LiveFacts);
    let cases: [(&str, &str, Mutation); 9] = [
        ("G7", "point_occluded", |l| l.point_unoccluded = false),
        ("G8", "target_not_frontmost", |l| l.target_frontmost = false),
        ("G8", "focus_changed", |l| l.focus_unchanged = false),
        ("G9", "hid_not_idle", |l| l.hid_idle = false),
        ("G9", "modifier_held", |l| l.modifiers_released = false),
        ("G9", "caps_lock_on", |l| l.caps_lock_off = false),
        ("G9", "mouse_button_down", |l| {
            l.mouse_buttons_released = false
        }),
        ("G9", "secure_input_enabled", |l| l.secure_input_off = false),
        ("G9", "input_source_unexpected", |l| {
            l.input_source_ok = false
        }),
    ];
    for (g, reason, m) in cases {
        let mut gate = gate_armed(now);
        let mut l = live();
        m(&mut l);
        let (mut sink, mut fake) = (Sink::default(), Fake::default());
        let out = submit_with(&mut gate, &click_json(), now, &l, &mut sink, &mut fake);
        refused(&out, g, reason);
        assert!(fake.calls.is_empty() && sink.attempts.is_empty());
        // A refusal does not consume an event.
        assert_eq!(gate.remaining_events(), Some(3));
    }
}

#[test]
fn occlusion_does_not_matter_for_key_taps() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let mut l = live();
    l.point_unoccluded = false;
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    let v = request_json(json!({"kind": "key_tap", "keycode": 49}));
    let out = submit_with(&mut gate, &v, now, &l, &mut sink, &mut fake);
    assert_eq!(out.status, Status::PostedUnverified);
}

// ---- fake backend and receipts ----

#[test]
fn each_event_kind_posts_and_writes_attempt_then_result() {
    let now = Instant::now();
    let cases = [
        (
            json!({"kind": "pointer_move", "x": 5.0, "y": 66.0}),
            vec!["move(5,66)"],
        ),
        (
            json!({"kind": "click", "button": "left", "x": 10.0, "y": 80.0}),
            vec!["down(10,80)", "up(10,80)"],
        ),
        (
            json!({"kind": "key_tap", "keycode": 36}),
            vec!["keydown(36)", "keyup(36)"],
        ),
    ];
    for (event, expected) in cases {
        let mut gate = gate_armed(now);
        let (out, sink, fake) = submit(&mut gate, &request_json(event), now);
        assert_eq!(out.status, Status::PostedUnverified, "{out:?}");
        assert!(out.attempted);
        assert_eq!(fake.calls, expected);
        assert_eq!(sink.attempts.len(), 1);
        assert_eq!(sink.results.len(), 1);
        assert_eq!(sink.attempts[0]["schema"], ATTEMPT_SCHEMA);
        assert_eq!(sink.results[0]["schema"], RESULT_SCHEMA);
        assert_eq!(sink.results[0]["status"], "posted_unverified");
        assert_eq!(sink.results[0]["id"], "r1");
        assert_eq!(gate.remaining_events(), Some(2));
        // The reply never authorizes a next action.
        assert_eq!(out.to_json()["action_allowed"], false);
    }
}

#[test]
fn attempt_record_binds_the_request_and_precedes_delivery() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (_, sink, _) = submit(&mut gate, &click_json(), now);
    let a = &sink.attempts[0];
    assert_eq!(a["id"], "r1");
    assert_eq!(a["event"]["kind"], "click");
    assert_eq!(a["target"]["bundle_id"], SAFARI);
    assert_eq!(a["target"]["pid"], 42);
    assert_eq!(a["observation_sha256"], sha());
    assert_eq!(a["delivery"], "foreground");
    assert_eq!(a["evidence_dir"], "/tmp/reach-evidence/new");
}

#[test]
fn failure_after_press_attempts_release_once_and_reports_unknown() {
    let now = Instant::now();
    // Click: `down` goes out and `up` is the failing call. It is attempted
    // exactly once and not repeated.
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("up");
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    assert!(out.attempted && out.release_attempted && !out.release_confirmed);
    assert_eq!(fake.calls, ["down(10,80)", "up(10,80)"], "no retry of up");

    // Key down fails: release is still attempted exactly once.
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("keydown");
    let v = request_json(json!({"kind": "key_tap", "keycode": 36}));
    let out = submit_with(&mut gate, &v, now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    assert!(out.release_attempted && out.release_confirmed);
    assert_eq!(fake.calls, ["keydown(36)", "keyup(36)"]);
    assert_eq!(sink.results[0]["status"], "delivery_unknown");
    assert_eq!(sink.results[0]["release_attempted"], true);
}

#[test]
fn failure_after_button_down_releases_the_button() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("down");
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    assert!(out.release_attempted && out.release_confirmed);
    assert_eq!(fake.calls, ["down(10,80)", "up(10,80)"]);
}

#[test]
fn pointer_move_failure_has_nothing_to_release() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("move");
    let v = request_json(json!({"kind": "pointer_move", "x": 1.0, "y": 61.0}));
    let out = submit_with(&mut gate, &v, now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    assert!(!out.release_attempted);
    assert_eq!(fake.calls, ["move(1,61)"]);
}

#[test]
fn unwritable_attempt_means_nothing_is_delivered() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    sink.fail_attempt = true;
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    refused(&out, "G12", "attempt_not_recorded");
    assert!(fake.calls.is_empty());
    assert!(sink.results.is_empty());
    assert_eq!(gate.remaining_events(), Some(3));
}

#[test]
fn unwritable_result_after_delivery_is_delivery_unknown() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    sink.fail_result = true;
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    assert_eq!(out.reason.as_deref(), Some("result_not_recorded"));
    assert!(out.attempted);
    assert_eq!(fake.calls.len(), 2);
    assert_eq!(sink.attempts.len(), 1);
}

#[test]
fn failed_post_check_is_delivery_unknown_not_posted() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_verify = true;
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    assert_eq!(out.gate, Some("G3"));
    assert_eq!(sink.results[0]["status"], "delivery_unknown");
}

#[test]
fn delivery_unknown_halts_the_gate_and_nothing_is_retried() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (mut sink, mut fake) = (Sink::default(), Fake::default());
    fake.fail_on = Some("up");
    let out = submit_with(&mut gate, &click_json(), now, &live(), &mut sink, &mut fake);
    assert_eq!(out.status, Status::DeliveryUnknown);
    let calls_after_first = fake.calls.len();
    // The same request again, with a healthy backend, is refused.
    let (out, sink2, fake2) = submit(&mut gate, &click_json(), now);
    refused(&out, "G13", "halted_after_delivery_unknown");
    assert!(fake2.calls.is_empty() && sink2.attempts.is_empty());
    assert_eq!(calls_after_first, 2);
    assert_eq!(gate.remaining_events(), Some(2));
}

#[test]
fn missing_result_is_read_back_as_delivery_unknown() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let (_, sink, _) = submit(&mut gate, &click_json(), now);
    let attempt = &sink.attempts[0];
    let result = &sink.results[0];
    assert_eq!(recover_status(None, None), RecoveredStatus::NotAttempted);
    assert_eq!(
        recover_status(Some(attempt), None),
        RecoveredStatus::Reported(Status::DeliveryUnknown)
    );
    assert_eq!(
        recover_status(Some(attempt), Some(&json!({"schema": "junk"}))),
        RecoveredStatus::Reported(Status::DeliveryUnknown)
    );
    assert_eq!(
        recover_status(
            Some(attempt),
            Some(&json!({"schema": RESULT_SCHEMA, "id": "other", "status": "posted_unverified"}))
        ),
        RecoveredStatus::Reported(Status::DeliveryUnknown)
    );
    assert_eq!(
        recover_status(Some(attempt), Some(result)),
        RecoveredStatus::Reported(Status::PostedUnverified)
    );
    // A result without its attempt is not trusted.
    assert_eq!(
        recover_status(None, Some(result)),
        RecoveredStatus::Reported(Status::DeliveryUnknown)
    );
}

#[test]
fn outcome_json_names_gate_and_reason_for_refusals() {
    let now = Instant::now();
    let mut gate = gate_armed(now);
    let mut v = click_json();
    v["target"]["bundle_id"] = json!("com.google.Chrome");
    let (out, _, _) = submit(&mut gate, &v, now);
    let j = out.to_json();
    assert_eq!(j["status"], "refused");
    assert_eq!(j["result"]["gate"], "G2");
    assert_eq!(j["result"]["reason"], "bundle_not_allowed");
    assert_eq!(j["result"]["attempted"], false);
    assert_eq!(j["action_allowed"], false);
}
