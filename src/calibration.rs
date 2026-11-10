//! Offline coordinate calibration fit. No UI calls, no image decoding, no input.
//! A fitted calibration only describes marker measurements; it never authorizes input.
use crate::Rect;
use serde_json::{json, Value};
use std::time::Instant;

pub const MIN_MARKERS: usize = 3;
pub const MAX_MARKERS: usize = 64;
pub const MAX_RESIDUAL_PX: f64 = 1.5;
pub const MAX_SCALE_REL_ERROR: f64 = 0.01;
pub const MAX_ORIGIN_ERROR_PX: f64 = 2.0;
pub const MIN_NONCOLLINEAR_AREA: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub point: (f64, f64),
    pub pixel: (f64, f64),
}

#[derive(Debug, Clone)]
pub struct CalibrationInput {
    pub markers: Vec<Marker>,
    pub window_frame: Rect,
    pub point_pixel_scale: f64,
    pub display_sha256: String,
    pub sampled_at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    pub scale_x: f64,
    pub offset_x: f64,
    pub scale_y: f64,
    pub offset_y: f64,
    pub residuals_px: Vec<f64>,
    pub max_residual_px: f64,
}

#[derive(Debug, Clone)]
pub struct Calibration {
    pub fit: Fit,
    pub window_frame: Rect,
    pub point_pixel_scale: f64,
    pub display_sha256: String,
    pub calibrated_at: Instant,
    pub evidence_id: String,
    marker_count: usize,
}

#[derive(Debug, Clone)]
pub enum CalibrationOutcome {
    Fitted(Calibration),
    Unknown(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct CurrentContext<'a> {
    pub display_sha256: &'a str,
    pub window_frame: Rect,
    pub point_pixel_scale: f64,
    pub now: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    NotInvalidated,
    Invalid(&'static str),
}

fn finite_pair(p: (f64, f64)) -> bool {
    p.0.is_finite() && p.1.is_finite()
}
/// Fail-closed bound check: NaN or infinity is never within a limit.
fn within(value: f64, limit: f64) -> bool {
    value.is_finite() && value <= limit
}
fn valid_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
/// Least squares for `p = scale * v + offset`. The caller guarantees `v` is not constant.
fn axis_fit(values: &[f64], pixels: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean_v = values.iter().sum::<f64>() / n;
    let mean_p = pixels.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (v, p) in values.iter().zip(pixels) {
        num += (v - mean_v) * (p - mean_p);
        den += (v - mean_v) * (v - mean_v);
    }
    let scale = num / den;
    (scale, mean_p - scale * mean_v)
}

/// Pure arithmetic: no hashing, so it behaves the same on every platform.
pub fn fit(input: &CalibrationInput) -> Result<Fit, &'static str> {
    let markers = &input.markers;
    if markers.len() > MAX_MARKERS {
        return Err("too_many_markers");
    }
    if markers.len() < MIN_MARKERS {
        return Err("markers_insufficient");
    }
    let f = input.window_frame;
    if !markers
        .iter()
        .all(|m| finite_pair(m.point) && finite_pair(m.pixel))
        || ![f.x, f.y, f.width, f.height, input.point_pixel_scale]
            .into_iter()
            .all(f64::is_finite)
    {
        return Err("non_finite_input");
    }
    let scale = input.point_pixel_scale;
    if scale <= 0.0 || scale > 16.0 {
        return Err("invalid_scale");
    }
    if f.width <= 0.0 || f.height <= 0.0 {
        return Err("invalid_window_frame");
    }
    if !valid_digest(&input.display_sha256) {
        return Err("invalid_display_digest");
    }
    if markers.iter().any(|m| {
        m.point.0 < f.x
            || m.point.0 > f.x + f.width
            || m.point.1 < f.y
            || m.point.1 > f.y + f.height
    }) {
        return Err("marker_outside_window");
    }
    if markers.iter().any(|m| m.pixel.0 < 0.0 || m.pixel.1 < 0.0) {
        return Err("marker_pixel_negative");
    }
    let p0 = markers[0].point;
    let cross = |a: &Marker, b: &Marker| {
        ((a.point.0 - p0.0) * (b.point.1 - p0.1) - (a.point.1 - p0.1) * (b.point.0 - p0.0)).abs()
    };
    let noncollinear = markers[1..].iter().any(|a| {
        markers[1..]
            .iter()
            .any(|b| cross(a, b) >= MIN_NONCOLLINEAR_AREA)
    });
    if !noncollinear {
        return Err("markers_collinear");
    }
    let xs: Vec<f64> = markers.iter().map(|m| m.point.0).collect();
    let ys: Vec<f64> = markers.iter().map(|m| m.point.1).collect();
    let pxs: Vec<f64> = markers.iter().map(|m| m.pixel.0).collect();
    let pys: Vec<f64> = markers.iter().map(|m| m.pixel.1).collect();
    let (scale_x, offset_x) = axis_fit(&xs, &pxs);
    let (scale_y, offset_y) = axis_fit(&ys, &pys);
    // Finite inputs can still overflow the sums; never report such a fit.
    if ![scale_x, offset_x, scale_y, offset_y]
        .into_iter()
        .all(f64::is_finite)
    {
        return Err("non_finite_input");
    }
    let residuals_px: Vec<f64> = markers
        .iter()
        .map(|m| {
            let dx = m.pixel.0 - (scale_x * m.point.0 + offset_x);
            let dy = m.pixel.1 - (scale_y * m.point.1 + offset_y);
            dx.hypot(dy)
        })
        .collect();
    if !residuals_px.iter().all(|r| r.is_finite()) {
        return Err("non_finite_input");
    }
    let max_residual_px = residuals_px.iter().copied().fold(0.0, f64::max);
    // Written fail-closed: a NaN comparison must reject, not pass.
    if !within(max_residual_px, MAX_RESIDUAL_PX) {
        return Err("residual_exceeds_threshold");
    }
    if !within((scale_x / scale - 1.0).abs(), MAX_SCALE_REL_ERROR)
        || !within((scale_y / scale - 1.0).abs(), MAX_SCALE_REL_ERROR)
    {
        return Err("fitted_scale_mismatch");
    }
    let origin = (scale_x * f.x + offset_x, scale_y * f.y + offset_y);
    if !within(origin.0.hypot(origin.1), MAX_ORIGIN_ERROR_PX) {
        return Err("frame_origin_mismatch");
    }
    Ok(Fit {
        scale_x,
        offset_x,
        scale_y,
        offset_y,
        residuals_px,
        max_residual_px,
    })
}

fn rect_json(r: Rect) -> Value {
    json!({"x":r.x,"y":r.y,"width":r.width,"height":r.height})
}

const INVALIDATED_BY: [&str; 4] = [
    "display_changed",
    "window_frame_changed",
    "scale_changed",
    "calibration_expired",
];

fn content_json(input: &CalibrationInput, fit: &Fit) -> Value {
    json!({"schema_version":1,"status":"calibration_fitted_unverified","action_allowed":false,
        "marker_count":input.markers.len(),"point_pixel_scale":input.point_pixel_scale,
        "window_frame_points":rect_json(input.window_frame),"display_sha256":input.display_sha256,
        "fit":{"scale_x":fit.scale_x,"offset_x":fit.offset_x,"scale_y":fit.scale_y,"offset_y":fit.offset_y},
        "residuals_px":fit.residuals_px,"max_residual_px":fit.max_residual_px,
        "max_residual_threshold_px":MAX_RESIDUAL_PX,
        "max_age_seconds":crate::ax_cg::MAX_CALIBRATION_AGE.as_secs(),"invalidated_by":INVALIDATED_BY,
        "note":"校準只描述擬合結果，不證明視窗綁定、遮擋或畫面新鮮度，也不授權輸入。"})
}

/// Fit and identify the evidence. The id hashes the content only, never the time.
pub fn calibrate(input: &CalibrationInput) -> CalibrationOutcome {
    let fit = match fit(input) {
        Ok(fit) => fit,
        Err(reason) => return CalibrationOutcome::Unknown(reason),
    };
    let Ok(bytes) = serde_json::to_vec(&content_json(input, &fit)) else {
        return CalibrationOutcome::Unknown("evidence_hash_unavailable");
    };
    let Ok(evidence_id) = crate::bytes_sha256(&bytes) else {
        return CalibrationOutcome::Unknown("evidence_hash_unavailable");
    };
    CalibrationOutcome::Fitted(Calibration {
        fit,
        window_frame: input.window_frame,
        point_pixel_scale: input.point_pixel_scale,
        display_sha256: input.display_sha256.clone(),
        calibrated_at: input.sampled_at,
        evidence_id,
        marker_count: input.markers.len(),
    })
}

impl CalibrationOutcome {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Fitted(c) => c.to_json(),
            Self::Unknown(reason) => {
                json!({"schema_version":1,"status":"unknown","reason":reason,"action_allowed":false})
            }
        }
    }
}

impl Calibration {
    /// Reports whether this calibration has been invalidated by the current state.
    pub fn check(&self, now: &CurrentContext<'_>) -> Validity {
        if now.display_sha256 != self.display_sha256 {
            return Validity::Invalid("display_changed");
        }
        if now.window_frame != self.window_frame {
            return Validity::Invalid("window_frame_changed");
        }
        if now.point_pixel_scale != self.point_pixel_scale {
            return Validity::Invalid("scale_changed");
        }
        match now.now.checked_duration_since(self.calibrated_at) {
            None => Validity::Invalid("calibration_time_invalid"),
            Some(age) if age > crate::ax_cg::MAX_CALIBRATION_AGE => {
                Validity::Invalid("calibration_expired")
            }
            Some(_) => Validity::NotInvalidated,
        }
    }
    pub fn check_json(&self, now: &CurrentContext<'_>) -> Value {
        match self.check(now) {
            Validity::NotInvalidated => {
                json!({"schema_version":1,"status":"not_invalidated_unverified",
                "evidence_id":self.evidence_id,"action_allowed":false})
            }
            Validity::Invalid(reason) => {
                json!({"schema_version":1,"status":"invalidated","reason":reason,
                "evidence_id":self.evidence_id,"action_allowed":false})
            }
        }
    }
    pub fn to_json(&self) -> Value {
        let mut v = content_json(
            &CalibrationInput {
                markers: Vec::new(),
                window_frame: self.window_frame,
                point_pixel_scale: self.point_pixel_scale,
                display_sha256: self.display_sha256.clone(),
                sampled_at: self.calibrated_at,
            },
            &self.fit,
        );
        v["marker_count"] = json!(self.marker_count);
        v["evidence_id"] = json!(self.evidence_id);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }
    fn input(frame: Rect, scale: f64, pts: &[(f64, f64)]) -> CalibrationInput {
        CalibrationInput {
            markers: pts
                .iter()
                .map(|&(x, y)| Marker {
                    point: (x, y),
                    pixel: (scale * (x - frame.x), scale * (y - frame.y)),
                })
                .collect(),
            window_frame: frame,
            point_pixel_scale: scale,
            display_sha256: "a".repeat(64),
            sampled_at: Instant::now(),
        }
    }
    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }
    fn frame_a() -> Rect {
        rect(100.0, 50.0, 400.0, 300.0)
    }
    const PTS_A: [(f64, f64); 4] = [(110.0, 60.0), (490.0, 60.0), (110.0, 340.0), (300.0, 200.0)];

    #[test]
    fn fits_scale_one() {
        let f = fit(&input(frame_a(), 1.0, &PTS_A)).unwrap();
        assert!(near(f.scale_x, 1.0) && near(f.scale_y, 1.0));
        assert!(f.max_residual_px < 1e-9);
    }
    #[test]
    fn fits_scale_two() {
        let f = fit(&input(frame_a(), 2.0, &PTS_A)).unwrap();
        assert!(near(f.scale_x, 2.0) && near(f.scale_y, 2.0));
        assert!(near(f.offset_x, -200.0) && near(f.offset_y, -100.0));
        assert_eq!(f.residuals_px.len(), 4);
    }
    #[test]
    fn fits_fractional_scale() {
        let pts = [(10.0, 10.0), (190.0, 10.0), (10.0, 90.0), (100.0, 50.0)];
        let f = fit(&input(rect(0.0, 0.0, 200.0, 100.0), 1.5, &pts)).unwrap();
        assert!(near(f.scale_x, 1.5) && near(f.scale_y, 1.5));
    }
    #[test]
    fn negative_origin_left_display() {
        let pts = [
            (-1092.0, 10.0),
            (-12.0, 10.0),
            (-1092.0, 732.0),
            (-600.0, 300.0),
        ];
        let i = input(rect(-1102.0, 0.0, 1102.0, 742.0), 2.0, &pts);
        assert_eq!(i.markers[2].pixel, (20.0, 1464.0));
        let f = fit(&i).unwrap();
        assert!(near(f.scale_x, 2.0) && near(f.scale_y, 2.0));
        assert!(near(f.offset_x, 2204.0) && near(f.offset_y, 0.0));
    }
    #[test]
    fn negative_origin_display_above() {
        let pts = [
            (10.0, -790.0),
            (590.0, -790.0),
            (10.0, -10.0),
            (300.0, -400.0),
        ];
        let f = fit(&input(rect(0.0, -800.0, 600.0, 800.0), 2.0, &pts)).unwrap();
        assert!(near(f.scale_x, 2.0) && near(f.scale_y, 2.0));
        assert!(near(f.offset_x, 0.0) && near(f.offset_y, 1600.0));
    }
    #[test]
    fn two_markers_are_insufficient() {
        let i = input(frame_a(), 2.0, &PTS_A[..2]);
        assert_eq!(fit(&i), Err("markers_insufficient"));
    }
    #[test]
    fn collinear_markers_are_rejected() {
        let i = input(
            rect(0.0, 0.0, 100.0, 100.0),
            2.0,
            &[(10.0, 10.0), (20.0, 20.0), (30.0, 30.0)],
        );
        assert_eq!(fit(&i), Err("markers_collinear"));
    }
    #[test]
    fn duplicate_markers_are_collinear() {
        let i = input(frame_a(), 2.0, &[PTS_A[0], PTS_A[0], PTS_A[0], PTS_A[0]]);
        assert_eq!(fit(&i), Err("markers_collinear"));
    }
    #[test]
    fn non_finite_is_rejected() {
        let mut i = input(frame_a(), 2.0, &PTS_A);
        i.markers[1].pixel.0 = f64::NAN;
        assert_eq!(fit(&i), Err("non_finite_input"));
        type Change = fn(&mut CalibrationInput);
        let changes: [(&str, Change); 4] = [
            ("point", |i| i.markers[0].point.1 = f64::INFINITY),
            ("frame", |i| i.window_frame.width = f64::INFINITY),
            ("scale", |i| i.point_pixel_scale = f64::INFINITY),
            ("frame_x", |i| i.window_frame.x = f64::NEG_INFINITY),
        ];
        for (name, change) in changes {
            let mut i = input(frame_a(), 2.0, &PTS_A);
            change(&mut i);
            assert_eq!(fit(&i), Err("non_finite_input"), "{name}");
        }
    }
    #[test]
    fn finite_inputs_that_overflow_the_fit_are_unknown() {
        let big = 1e200;
        let frame = rect(0.0, 0.0, big, big);
        let i = CalibrationInput {
            markers: [(0.0, 0.0), (big, 0.0), (0.0, big)]
                .iter()
                .map(|&p| Marker { point: p, pixel: p })
                .collect(),
            window_frame: frame,
            point_pixel_scale: 1.0,
            display_sha256: "a".repeat(64),
            sampled_at: Instant::now(),
        };
        assert_eq!(fit(&i), Err("non_finite_input"));
        assert!(matches!(
            calibrate(&i),
            CalibrationOutcome::Unknown("non_finite_input")
        ));
    }
    #[test]
    fn invalid_scale() {
        for s in [0.0, 20.0] {
            let mut i = input(frame_a(), 2.0, &PTS_A);
            i.point_pixel_scale = s;
            assert_eq!(fit(&i), Err("invalid_scale"), "{s}");
        }
    }
    #[test]
    fn bad_display_digest() {
        for d in ["A".repeat(64), "a".repeat(63)] {
            let mut i = input(frame_a(), 2.0, &PTS_A);
            i.display_sha256 = d.clone();
            assert_eq!(fit(&i), Err("invalid_display_digest"), "{d}");
        }
    }
    #[test]
    fn marker_outside_window() {
        let mut i = input(frame_a(), 2.0, &PTS_A);
        i.markers[0].point = (99.0, 60.0);
        assert_eq!(fit(&i), Err("marker_outside_window"));
    }
    #[test]
    fn outlier_exceeds_residual_threshold() {
        let mut i = input(frame_a(), 1.0, &PTS_A);
        i.markers[3].pixel.0 += 20.0;
        assert_eq!(fit(&i), Err("residual_exceeds_threshold"));
    }
    #[test]
    fn declared_scale_must_match_fit() {
        let mut i = input(frame_a(), 2.0, &PTS_A);
        i.point_pixel_scale = 1.5;
        assert_eq!(fit(&i), Err("fitted_scale_mismatch"));
    }
    #[test]
    fn shifted_origin_is_rejected() {
        let mut i = input(frame_a(), 2.0, &PTS_A);
        for m in &mut i.markers {
            m.pixel.0 += 10.0;
            m.pixel.1 += 10.0;
        }
        assert_eq!(fit(&i), Err("frame_origin_mismatch"));
    }
    #[test]
    fn too_many_markers() {
        let pts: Vec<(f64, f64)> = (0..65)
            .map(|n| (110.0 + f64::from(n) * 5.0, 60.0 + f64::from(n % 7) * 30.0))
            .collect();
        assert_eq!(fit(&input(frame_a(), 2.0, &pts)), Err("too_many_markers"));
    }
    #[test]
    fn image_size_agreement_without_markers_is_unknown() {
        let metadata = json!({"capture_type":"desktop_independent_window","point_pixel_scale":2,
            "window_frame_points":{"x":100,"y":50,"width":400,"height":300},
            "filter_content_rect_points":{"x":100,"y":50,"width":400,"height":300},
            "image_width":800,"image_height":600});
        assert_eq!(
            crate::capture_geometry::inspect(&metadata)["status"],
            "metadata_consistent_unverified"
        );
        let i = input(frame_a(), 2.0, &[]);
        assert_eq!(fit(&i), Err("markers_insufficient"));
        assert!(matches!(
            calibrate(&i),
            CalibrationOutcome::Unknown("markers_insufficient")
        ));
    }

    #[cfg(target_os = "macos")]
    fn fitted(i: &CalibrationInput) -> Calibration {
        match calibrate(i) {
            CalibrationOutcome::Fitted(c) => c,
            CalibrationOutcome::Unknown(r) => panic!("unexpected unknown: {r}"),
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn calibrate_returns_evidence_id_and_json() {
        let c = fitted(&input(frame_a(), 2.0, &PTS_A));
        assert_eq!(c.evidence_id.len(), 64);
        assert!(c.evidence_id.bytes().all(|b| b.is_ascii_hexdigit()));
        let v = CalibrationOutcome::Fitted(c).to_json();
        assert_eq!(v["status"], "calibration_fitted_unverified");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["residuals_px"].as_array().unwrap().len(), 4);
        let names: Vec<&str> = v["invalidated_by"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            names,
            [
                "display_changed",
                "window_frame_changed",
                "scale_changed",
                "calibration_expired"
            ]
        );
        for forbidden in ["ax_path", "click", "target_point"] {
            assert!(v.get(forbidden).is_none(), "{forbidden}");
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn same_content_same_evidence_id() {
        let a = input(frame_a(), 2.0, &PTS_A);
        let mut b = a.clone();
        assert_eq!(fitted(&a).evidence_id, fitted(&b).evidence_id);
        b.markers[3].pixel.0 += 0.1;
        assert_ne!(fitted(&a).evidence_id, fitted(&b).evidence_id);
    }
    /// A calibration built without hashing, so its logic is tested on every platform.
    fn manual(i: &CalibrationInput) -> Calibration {
        Calibration {
            fit: fit(i).unwrap(),
            window_frame: i.window_frame,
            point_pixel_scale: i.point_pixel_scale,
            display_sha256: i.display_sha256.clone(),
            calibrated_at: i.sampled_at,
            evidence_id: "e".repeat(64),
            marker_count: i.markers.len(),
        }
    }
    #[test]
    fn invalidation_conditions() {
        let i = input(frame_a(), 2.0, &PTS_A);
        let t0 = i.sampled_at;
        let c = manual(&i);
        let digest = "a".repeat(64);
        let ctx = |display: &str, frame: Rect, scale: f64, secs: u64| {
            c.check(&CurrentContext {
                display_sha256: display,
                window_frame: frame,
                point_pixel_scale: scale,
                now: t0 + Duration::from_secs(secs),
            })
        };
        let f = frame_a();
        assert_eq!(ctx(&digest, f, 2.0, 10), Validity::NotInvalidated);
        assert_eq!(
            ctx(&"b".repeat(64), f, 2.0, 10),
            Validity::Invalid("display_changed")
        );
        let moved = rect(f.x + 1.0, f.y, f.width, f.height);
        let resized = rect(f.x, f.y, f.width + 1.0, f.height);
        assert_eq!(
            ctx(&digest, moved, 2.0, 10),
            Validity::Invalid("window_frame_changed")
        );
        assert_eq!(
            ctx(&digest, resized, 2.0, 10),
            Validity::Invalid("window_frame_changed")
        );
        assert_eq!(ctx(&digest, f, 1.5, 10), Validity::Invalid("scale_changed"));
        assert_eq!(ctx(&digest, f, 2.0, 60), Validity::NotInvalidated);
        assert_eq!(
            ctx(&digest, f, 2.0, 61),
            Validity::Invalid("calibration_expired")
        );
        let v = c.check_json(&CurrentContext {
            display_sha256: &digest,
            window_frame: f,
            point_pixel_scale: 2.0,
            now: t0 + Duration::from_secs(61),
        });
        assert_eq!(v["status"], "invalidated");
        assert_eq!(v["reason"], "calibration_expired");
        assert_eq!(v["action_allowed"], false);
        let ok = c.check_json(&CurrentContext {
            display_sha256: &digest,
            window_frame: f,
            point_pixel_scale: 2.0,
            now: t0,
        });
        assert_eq!(ok["status"], "not_invalidated_unverified");
        assert_eq!(ok["action_allowed"], false);
    }
    #[test]
    fn clock_earlier_than_calibration_is_invalid() {
        let mut i = input(frame_a(), 2.0, &PTS_A);
        i.sampled_at = Instant::now() + Duration::from_secs(5);
        let c = manual(&i);
        let digest = "a".repeat(64);
        let v = c.check(&CurrentContext {
            display_sha256: &digest,
            window_frame: frame_a(),
            point_pixel_scale: 2.0,
            now: Instant::now(),
        });
        assert_eq!(v, Validity::Invalid("calibration_time_invalid"));
    }
    #[test]
    fn to_json_is_portable_and_never_authorizes_input() {
        let v = manual(&input(frame_a(), 2.0, &PTS_A)).to_json();
        assert_eq!(v["status"], "calibration_fitted_unverified");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["marker_count"], 4);
        assert_eq!(v["residuals_px"].as_array().unwrap().len(), 4);
        assert_eq!(v["evidence_id"], "e".repeat(64));
        assert_eq!(v["max_age_seconds"], 60);
        for forbidden in ["ax_path", "click", "target_point"] {
            assert!(v.get(forbidden).is_none(), "{forbidden}");
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn evidence_id_hashes_the_json_without_the_id() {
        let c = fitted(&input(frame_a(), 2.0, &PTS_A));
        let mut v = c.to_json();
        assert_eq!(v["evidence_id"], c.evidence_id);
        v.as_object_mut().unwrap().remove("evidence_id");
        let hashed = crate::bytes_sha256(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert_eq!(hashed, c.evidence_id);
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn calibrate_without_native_sha_is_unknown() {
        assert!(matches!(
            calibrate(&input(frame_a(), 2.0, &PTS_A)),
            CalibrationOutcome::Unknown("evidence_hash_unavailable")
        ));
    }
}
