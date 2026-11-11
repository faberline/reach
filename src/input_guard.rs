//! Offline checks for a future window-input backend. This module sends no input.
//!
//! A passing check is not a capability or authorization to act. The caller must
//! independently establish the provenance of every observation and current
//! window identity. This module cannot establish whether a backend preserves the
//! user's pointer, focus, or window order, or whether an app accepts an event.
use crate::{Target, Window};
use std::time::{Duration, Instant};

/// A frame expires after this interval, measured on the local monotonic clock.
pub const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Window(Target),
    Desktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Background,
    Foreground,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPoint {
    /// Points from the left edge of the captured window, not screen coordinates.
    pub x: f64,
    /// Points from the top edge of the captured window, not screen coordinates.
    pub y: f64,
}

/// Evidence supplied by the caller. Construction does not verify its origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    window: Window,
    process_start_unix_seconds: f64,
    image_sha256: String,
    captured_at: Instant,
    sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Denial {
    InvalidObservation,
    DesktopUnsupported,
    ForegroundUnsupported,
    StaleObservation,
    ObservationExpired,
    CurrentIdentityMismatch,
    TargetMismatch,
    PointOutsideWindow,
}

impl Observation {
    pub(crate) fn capture_sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn captured_at(&self) -> Instant {
        self.captured_at
    }

    /// `sequence` must be unique to each new frame within the caller's session.
    /// The capture time must come from the same monotonic clock used by `review`.
    pub fn new(
        window: Window,
        process_start_unix_seconds: f64,
        image_sha256: String,
        captured_at: Instant,
        sequence: u64,
    ) -> Result<Self, Denial> {
        if Target::new(window.target.pid, window.target.window_id).is_err()
            || !window.bounds.valid()
            || window.layer != Some(0)
            || !process_start_unix_seconds.is_finite()
            || process_start_unix_seconds <= 0.0
            || image_sha256.len() != 64
            || !image_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || sequence == 0
        {
            return Err(Denial::InvalidObservation);
        }
        Ok(Self {
            window,
            process_start_unix_seconds,
            image_sha256,
            captured_at,
            sequence,
        })
    }

    pub fn image_sha256(&self) -> &str {
        &self.image_sha256
    }

    pub fn token(&self) -> ObservationToken {
        ObservationToken(self.clone())
    }
}

/// A copy of the observation used to select a point. Not an action capability.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationToken(Observation);

/// The caller must obtain this identity and geometry again immediately before use.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentWindow {
    pub window: Window,
    pub process_start_unix_seconds: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputRequest {
    pub destination: Destination,
    pub delivery: Delivery,
    pub point: WindowPoint,
    pub observation: ObservationToken,
}

/// Passing these checks still does not allow input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreflightOnly;
impl PreflightOnly {
    pub const fn action_allowed(self) -> bool {
        false
    }
}

/// Foreground variant of `InputRequest`. Delivery is implicitly foreground, and
/// `point` is `None` for events that carry no coordinates (a key tap).
#[derive(Debug, Clone, PartialEq)]
pub struct ForegroundRequest {
    pub destination: Destination,
    pub point: Option<WindowPoint>,
    pub observation: ObservationToken,
}

/// Pure comparison only. It does not inspect the OS or run an input backend.
pub fn review(
    request: &InputRequest,
    latest: &Observation,
    current: &CurrentWindow,
    now: Instant,
) -> Result<PreflightOnly, Denial> {
    if request.delivery != Delivery::Background {
        return Err(Denial::ForegroundUnsupported);
    }
    review_common(
        request.destination,
        Some(request.point),
        &request.observation,
        latest,
        current,
        now,
    )
}

/// Foreground variant of `review`: the same identity, freshness and geometry
/// checks, with `Desktop` still refused. Pure comparison; it grants nothing.
/// Foreground-only conditions (focus, HID state, ...) are not decided here.
pub fn review_foreground(
    request: &ForegroundRequest,
    latest: &Observation,
    current: &CurrentWindow,
    now: Instant,
) -> Result<PreflightOnly, Denial> {
    review_common(
        request.destination,
        request.point,
        &request.observation,
        latest,
        current,
        now,
    )
}

fn review_common(
    destination: Destination,
    point: Option<WindowPoint>,
    observation: &ObservationToken,
    latest: &Observation,
    current: &CurrentWindow,
    now: Instant,
) -> Result<PreflightOnly, Denial> {
    let target = match destination {
        Destination::Window(target) => target,
        Destination::Desktop => return Err(Denial::DesktopUnsupported),
    };
    if observation.0 != *latest {
        return Err(Denial::StaleObservation);
    }
    let age = now
        .checked_duration_since(latest.captured_at)
        .ok_or(Denial::ObservationExpired)?;
    if age > MAX_OBSERVATION_AGE {
        return Err(Denial::ObservationExpired);
    }
    if target != latest.window.target {
        return Err(Denial::TargetMismatch);
    }
    if current.window != latest.window
        || current.process_start_unix_seconds != latest.process_start_unix_seconds
    {
        return Err(Denial::CurrentIdentityMismatch);
    }
    let Some(p) = point else {
        return Ok(PreflightOnly);
    };
    if !p.x.is_finite()
        || !p.y.is_finite()
        || p.x < 0.0
        || p.y < 0.0
        || p.x >= latest.window.bounds.width
        || p.y >= latest.window.bounds.height
        || !(latest.window.bounds.x + p.x).is_finite()
        || !(latest.window.bounds.y + p.y).is_finite()
    {
        return Err(Denial::PointOutsideWindow);
    }
    Ok(PreflightOnly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rect;

    fn frame(at: Instant, sequence: u64) -> Observation {
        Observation::new(
            Window {
                target: Target::new(42, 7).unwrap(),
                layer: Some(0),
                title: None,
                bounds: Rect {
                    x: -200.0,
                    y: 20.0,
                    width: 100.0,
                    height: 80.0,
                },
            },
            123.0,
            "a".repeat(64),
            at,
            sequence,
        )
        .unwrap()
    }

    fn context(frame: &Observation) -> (InputRequest, CurrentWindow) {
        (
            InputRequest {
                destination: Destination::Window(frame.window.target),
                delivery: Delivery::Background,
                point: WindowPoint { x: 20.0, y: 10.0 },
                observation: frame.token(),
            },
            CurrentWindow {
                window: frame.window.clone(),
                process_start_unix_seconds: frame.process_start_unix_seconds,
            },
        )
    }

    #[test]
    fn valid_preflight_still_never_authorizes_input() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (request, current) = context(&latest);
        assert!(!review(&request, &latest, &current, now)
            .unwrap()
            .action_allowed());
    }

    #[test]
    fn rejects_desktop_and_foreground() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (mut request, current) = context(&latest);
        request.destination = Destination::Desktop;
        assert_eq!(
            review(&request, &latest, &current, now),
            Err(Denial::DesktopUnsupported)
        );
        request.destination = Destination::Window(latest.window.target);
        request.delivery = Delivery::Foreground;
        assert_eq!(
            review(&request, &latest, &current, now),
            Err(Denial::ForegroundUnsupported)
        );
    }

    #[test]
    fn rejects_stale_changed_and_expired_observations() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (request, current) = context(&latest);
        assert_eq!(
            review(&request, &frame(now, 2), &current, now),
            Err(Denial::StaleObservation)
        );
        let mut changed = latest.clone();
        changed.image_sha256 = "b".repeat(64);
        assert_eq!(
            review(&request, &changed, &current, now),
            Err(Denial::StaleObservation)
        );
        assert_eq!(
            review(
                &request,
                &latest,
                &current,
                now + MAX_OBSERVATION_AGE + Duration::from_nanos(1)
            ),
            Err(Denial::ObservationExpired)
        );
        assert_eq!(
            review(&request, &latest, &current, now - Duration::from_nanos(1)),
            Err(Denial::ObservationExpired)
        );
    }

    #[test]
    fn rejects_wrong_target_process_and_window_geometry() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (mut request, mut current) = context(&latest);
        request.destination = Destination::Window(Target::new(43, 7).unwrap());
        assert_eq!(
            review(&request, &latest, &current, now),
            Err(Denial::TargetMismatch)
        );
        request.destination = Destination::Window(latest.window.target);
        current.process_start_unix_seconds += 1.0;
        assert_eq!(
            review(&request, &latest, &current, now),
            Err(Denial::CurrentIdentityMismatch)
        );
        current.process_start_unix_seconds = latest.process_start_unix_seconds;
        current.window.bounds.x += 1.0;
        assert_eq!(
            review(&request, &latest, &current, now),
            Err(Denial::CurrentIdentityMismatch)
        );
    }

    #[test]
    fn rejects_invalid_or_outside_points() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (mut request, current) = context(&latest);
        for point in [
            WindowPoint { x: -1.0, y: 1.0 },
            WindowPoint { x: 100.0, y: 1.0 },
            WindowPoint { x: 0.0, y: 80.0 },
            WindowPoint {
                x: f64::NAN,
                y: 0.0,
            },
            WindowPoint {
                x: 0.0,
                y: f64::INFINITY,
            },
        ] {
            request.point = point;
            assert_eq!(
                review(&request, &latest, &current, now),
                Err(Denial::PointOutsideWindow)
            );
        }
        request.point = WindowPoint { x: 0.0, y: 0.0 };
        assert!(!review(&request, &latest, &current, now)
            .unwrap()
            .action_allowed());
    }

    #[test]
    fn rejects_unusable_observation_metadata() {
        let now = Instant::now();
        let valid = frame(now, 1);
        for hash in ["bad".into(), "A".repeat(64), "g".repeat(64)] {
            assert_eq!(
                Observation::new(valid.window.clone(), 123.0, hash, now, 1),
                Err(Denial::InvalidObservation)
            );
        }
        assert_eq!(
            Observation::new(valid.window.clone(), f64::NAN, "a".repeat(64), now, 1),
            Err(Denial::InvalidObservation)
        );
        assert_eq!(
            Observation::new(valid.window, 123.0, "a".repeat(64), now, 0),
            Err(Denial::InvalidObservation)
        );
    }

    #[test]
    fn rejects_unknown_and_non_normal_layers() {
        let now = Instant::now();
        let valid = frame(now, 1);
        for layer in [None, Some(-2147483603), Some(1)] {
            let mut window = valid.window.clone();
            window.layer = layer;
            assert_eq!(
                Observation::new(window, 123.0, "a".repeat(64), now, 1),
                Err(Denial::InvalidObservation)
            );
        }
        let (request, mut current) = context(&valid);
        current.window.layer = Some(1);
        assert_eq!(
            review(&request, &valid, &current, now),
            Err(Denial::CurrentIdentityMismatch)
        );
    }
    fn foreground(frame: &Observation, point: Option<WindowPoint>) -> ForegroundRequest {
        ForegroundRequest {
            destination: Destination::Window(frame.window.target),
            point,
            observation: frame.token(),
        }
    }

    #[test]
    fn foreground_review_accepts_what_background_review_refuses_by_delivery() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (_, current) = context(&latest);
        let ok = foreground(&latest, Some(WindowPoint { x: 20.0, y: 10.0 }));
        assert!(!review_foreground(&ok, &latest, &current, now)
            .unwrap()
            .action_allowed());
        // The background review keeps refusing foreground delivery.
        let (mut request, _) = context(&latest);
        request.delivery = Delivery::Foreground;
        assert_eq!(
            review(&request, &latest, &current, now),
            Err(Denial::ForegroundUnsupported)
        );
        // A key tap has no point.
        assert!(review_foreground(&foreground(&latest, None), &latest, &current, now).is_ok());
    }

    #[test]
    fn foreground_review_keeps_every_other_check() {
        let now = Instant::now();
        let latest = frame(now, 1);
        let (_, mut current) = context(&latest);
        let mut req = foreground(&latest, Some(WindowPoint { x: 20.0, y: 10.0 }));
        req.destination = Destination::Desktop;
        assert_eq!(
            review_foreground(&req, &latest, &current, now),
            Err(Denial::DesktopUnsupported)
        );
        req.destination = Destination::Window(Target::new(43, 7).unwrap());
        assert_eq!(
            review_foreground(&req, &latest, &current, now),
            Err(Denial::TargetMismatch)
        );
        let mut req = foreground(&latest, Some(WindowPoint { x: 100.0, y: 10.0 }));
        assert_eq!(
            review_foreground(&req, &latest, &current, now),
            Err(Denial::PointOutsideWindow)
        );
        req.point = None;
        req.observation = frame(now, 2).token();
        assert_eq!(
            review_foreground(&req, &latest, &current, now),
            Err(Denial::StaleObservation)
        );
        assert_eq!(
            review_foreground(
                &foreground(&latest, None),
                &latest,
                &current,
                now + MAX_OBSERVATION_AGE + Duration::from_nanos(1)
            ),
            Err(Denial::ObservationExpired)
        );
        current.process_start_unix_seconds += 1.0;
        assert_eq!(
            review_foreground(&foreground(&latest, None), &latest, &current, now),
            Err(Denial::CurrentIdentityMismatch)
        );
    }
}
