use mac_computer_use::{
    input_guard::{
        CurrentWindow, Delivery, Denial, Destination, InputRequest, Observation, WindowPoint,
    },
    virtual_input::{Button, Intent, StopReason, VirtualSession},
    Rect, Target, Window,
};
use std::time::{Duration, Instant};

fn fixture(now: Instant) -> (VirtualSession, Observation, CurrentWindow, InputRequest) {
    let target = Target::new(42, 11).unwrap();
    let window = Window {
        target,
        bounds: Rect {
            x: -900.0,
            y: 10.0,
            width: 200.0,
            height: 100.0,
        },
        layer: Some(0),
        title: Some("test".into()),
    };
    let current = CurrentWindow {
        window: window.clone(),
        process_start_unix_seconds: 123.0,
    };
    let obs = Observation::new(window, 123.0, "a".repeat(64), now, 1).unwrap();
    let request = InputRequest {
        destination: Destination::Window(target),
        delivery: Delivery::Background,
        point: WindowPoint { x: 20.0, y: 30.0 },
        observation: obs.token(),
    };
    (
        VirtualSession::new(current.clone(), now).unwrap(),
        obs,
        current,
        request,
    )
}

#[test]
fn observation_rejects_window_endpoint_overflow() {
    let now = Instant::now();
    let (_, _, current, _) = fixture(now);
    for overflow_x in [true, false] {
        let mut window = current.window.clone();
        if overflow_x {
            window.bounds.x = f64::MAX;
            window.bounds.width = f64::MAX;
        } else {
            window.bounds.y = f64::MAX;
            window.bounds.height = f64::MAX;
        }
        assert_eq!(
            Observation::new(window, 123.0, "a".repeat(64), now, 1),
            Err(Denial::InvalidObservation)
        );
    }
}

#[test]
fn accepted_window_points_have_finite_hypothetical_sums() {
    let now = Instant::now();
    let (_, _, mut current, _) = fixture(now);
    current.window.bounds = Rect {
        x: f64::MAX / 4.0,
        y: -f64::MAX / 4.0,
        width: f64::MAX / 4.0,
        height: f64::MAX / 4.0,
    };
    let obs = Observation::new(current.window.clone(), 123.0, "a".repeat(64), now, 1).unwrap();
    let mut request = InputRequest {
        destination: Destination::Window(current.window.target),
        delivery: Delivery::Background,
        point: WindowPoint {
            x: f64::MAX / 8.0,
            y: f64::MAX / 8.0,
        },
        observation: obs.token(),
    };
    let mut session = VirtualSession::new(current.clone(), now).unwrap();
    let plan = session
        .plan(1, &request, &obs, &current, now, Intent::Move)
        .unwrap();
    assert!((current.window.bounds.x + plan.point_in_window_points.x).is_finite());
    assert!((current.window.bounds.y + plan.point_in_window_points.y).is_finite());
    assert!(!plan.action_allowed());
    request.point.x = f64::MAX;
    assert_eq!(
        session.plan(2, &request, &obs, &current, now, Intent::Move),
        Err(StopReason::Preflight(Denial::PointOutsideWindow))
    );
}

#[test]
fn plans_drag_and_keyboard_without_authorizing_input() {
    let now = Instant::now();
    let (mut session, obs, current, mut req) = fixture(now);
    for (seq, intent) in [
        (1, Intent::Move),
        (2, Intent::ButtonDown(Button::Left)),
        (3, Intent::KeyDown(56)),
    ] {
        let p = session
            .plan(seq, &req, &obs, &current, now, intent)
            .unwrap();
        assert!(!p.action_allowed());
        assert_eq!(p.target, current.window.target);
    }
    req.point = WindowPoint { x: 80.0, y: 50.0 };
    for (seq, intent) in [
        (4, Intent::Move),
        (5, Intent::ButtonUp(Button::Left)),
        (6, Intent::KeyUp(56)),
    ] {
        assert!(!session
            .plan(seq, &req, &obs, &current, now, intent)
            .unwrap()
            .action_allowed());
    }
    assert!(!session.action_allowed());
    assert_eq!(session.finish(), Ok(()));
    assert_eq!(
        session.plan(7, &req, &obs, &current, now, Intent::Move),
        Err(StopReason::AlreadyStopped)
    );
}

#[test]
fn plans_preserve_held_button_and_modifier_transitions() {
    let now = Instant::now();
    let (mut s, obs, cur, req) = fixture(now);
    s.plan(1, &req, &obs, &cur, now, Intent::Move).unwrap();
    let down = s
        .plan(2, &req, &obs, &cur, now, Intent::ButtonDown(Button::Left))
        .unwrap();
    assert_eq!(down.button_before, None);
    assert_eq!(down.button_after, Some(Button::Left));
    let key = s
        .plan(3, &req, &obs, &cur, now, Intent::KeyDown(56))
        .unwrap();
    assert_eq!(key.keys_before, [0, 0]);
    assert_eq!(key.keys_after, [1_u64 << 56, 0]);
    let drag = s.plan(4, &req, &obs, &cur, now, Intent::Move).unwrap();
    assert_eq!(drag.button_before, Some(Button::Left));
    assert_eq!(drag.keys_before, [1_u64 << 56, 0]);
    let up = s
        .plan(5, &req, &obs, &cur, now, Intent::ButtonUp(Button::Left))
        .unwrap();
    assert_eq!(up.button_after, None);
    let keyup = s.plan(6, &req, &obs, &cur, now, Intent::KeyUp(56)).unwrap();
    assert_eq!(keyup.keys_before, [1_u64 << 56, 0]);
    assert_eq!(keyup.keys_after, [0, 0]);
}

#[test]
fn invalid_sequence_or_held_input_stops_session() {
    let now = Instant::now();
    let (mut s, obs, cur, req) = fixture(now);
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::ButtonUp(Button::Left)),
        Err(StopReason::ButtonNotHeld)
    );
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::Move),
        Err(StopReason::AlreadyStopped)
    );
    let (mut s, obs, cur, req) = fixture(now);
    s.plan(1, &req, &obs, &cur, now, Intent::Move).unwrap();
    s.plan(2, &req, &obs, &cur, now, Intent::ButtonDown(Button::Left))
        .unwrap();
    assert_eq!(s.finish(), Err(StopReason::InputsStillHeld));
    assert_eq!(
        s.plan(3, &req, &obs, &cur, now, Intent::ButtonUp(Button::Left)),
        Err(StopReason::AlreadyStopped)
    );
    let (mut s, obs, cur, req) = fixture(now);
    assert_eq!(
        s.plan(2, &req, &obs, &cur, now, Intent::Move),
        Err(StopReason::Sequence)
    );
}

#[test]
fn changed_target_stale_frame_and_foreground_are_terminal() {
    let now = Instant::now();
    let (mut s, obs, mut cur, req) = fixture(now);
    cur.process_start_unix_seconds += 1.0;
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::Move),
        Err(StopReason::TargetChanged)
    );
    let (mut s, obs, cur, mut req) = fixture(now);
    req.delivery = Delivery::Foreground;
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::Move),
        Err(StopReason::Preflight(Denial::ForegroundUnsupported))
    );
    let (mut s, obs, cur, req) = fixture(now);
    assert_eq!(
        s.plan(
            1,
            &req,
            &obs,
            &cur,
            now + Duration::from_secs(11),
            Intent::Move
        ),
        Err(StopReason::Preflight(Denial::ObservationExpired))
    );
    let (mut s, obs, cur, req) = fixture(now);
    let new = Observation::new(
        cur.window.clone(),
        cur.process_start_unix_seconds,
        "b".repeat(64),
        now,
        2,
    )
    .unwrap();
    assert_eq!(
        s.plan(1, &req, &new, &cur, now, Intent::Move),
        Err(StopReason::Preflight(Denial::StaleObservation))
    );
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::Move),
        Err(StopReason::AlreadyStopped)
    );
}

#[test]
fn rejects_unbalanced_buttons_invalid_keys_and_clock_rollback() {
    let now = Instant::now();
    let (mut s, obs, cur, req) = fixture(now);
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::ButtonUp(Button::Left)),
        Err(StopReason::ButtonNotHeld)
    );
    let (mut s, obs, cur, req) = fixture(now);
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::KeyDown(128)),
        Err(StopReason::InvalidKeyCode)
    );
    let (mut s, obs, cur, req) = fixture(now);
    s.plan(1, &req, &obs, &cur, now, Intent::KeyDown(0))
        .unwrap();
    assert_eq!(
        s.plan(2, &req, &obs, &cur, now, Intent::KeyDown(0)),
        Err(StopReason::KeyAlreadyHeld)
    );
    let (mut s, obs, cur, req) = fixture(now);
    assert_eq!(
        s.plan(
            1,
            &req,
            &obs,
            &cur,
            now - Duration::from_millis(1),
            Intent::Move
        ),
        Err(StopReason::ClockRollback)
    );
}

#[test]
fn click_without_move_is_balanced_and_boundaries_are_checked() {
    let now = Instant::now();
    let (mut s, obs, cur, req) = fixture(now);
    let down = s
        .plan(1, &req, &obs, &cur, now, Intent::ButtonDown(Button::Left))
        .unwrap();
    assert_eq!(down.point_in_window_points, req.point);
    assert!(!down.action_allowed());
    let up = s
        .plan(2, &req, &obs, &cur, now, Intent::ButtonUp(Button::Left))
        .unwrap();
    assert_eq!(up.button_after, None);
    assert!(!up.action_allowed());
    assert!(s.finish().is_ok());
    let (mut s, obs, cur, mut req) = fixture(now);
    s.plan(1, &req, &obs, &cur, now, Intent::ButtonDown(Button::Left))
        .unwrap();
    req.point.x += 1.0;
    assert_eq!(
        s.plan(2, &req, &obs, &cur, now, Intent::ButtonUp(Button::Left)),
        Err(StopReason::PositionUnknownOrChanged)
    );
    let (mut s, obs, cur, mut req) = fixture(now);
    req.point.x = 200.0;
    assert_eq!(
        s.plan(1, &req, &obs, &cur, now, Intent::Move),
        Err(StopReason::Preflight(Denial::PointOutsideWindow))
    );
}

#[test]
fn observation_sequence_cannot_regress_or_change_under_same_id() {
    let now = Instant::now();
    let (mut s, old, cur, mut req) = fixture(now);
    s.plan(1, &req, &old, &cur, now, Intent::Move).unwrap();
    let new = Observation::new(
        cur.window.clone(),
        cur.process_start_unix_seconds,
        "b".repeat(64),
        now + Duration::from_millis(1),
        2,
    )
    .unwrap();
    req.observation = new.token();
    s.plan(
        2,
        &req,
        &new,
        &cur,
        now + Duration::from_millis(1),
        Intent::KeyDown(0),
    )
    .unwrap();
    req.observation = old.token();
    assert_eq!(
        s.plan(
            3,
            &req,
            &old,
            &cur,
            now + Duration::from_millis(2),
            Intent::KeyUp(0)
        ),
        Err(StopReason::ObservationRegressed)
    );

    let (mut s, old, cur, mut req) = fixture(now);
    s.plan(1, &req, &old, &cur, now, Intent::Move).unwrap();
    let changed = Observation::new(
        cur.window.clone(),
        cur.process_start_unix_seconds,
        "b".repeat(64),
        now,
        1,
    )
    .unwrap();
    req.observation = changed.token();
    assert_eq!(
        s.plan(2, &req, &changed, &cur, now, Intent::KeyDown(0)),
        Err(StopReason::ObservationRegressed)
    );

    let (mut s, old, cur, mut req) = fixture(now);
    s.plan(1, &req, &old, &cur, now, Intent::Move).unwrap();
    let backwards = Observation::new(
        cur.window.clone(),
        cur.process_start_unix_seconds,
        "b".repeat(64),
        now - Duration::from_millis(1),
        2,
    )
    .unwrap();
    req.observation = backwards.token();
    assert_eq!(
        s.plan(2, &req, &backwards, &cur, now, Intent::KeyDown(0)),
        Err(StopReason::ObservationRegressed)
    );
}
