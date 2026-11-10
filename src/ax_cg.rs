//! Offline-only, caller-supplied AX/CG comparison. This never performs UI calls.
//!
//! A unique geometric match is only a candidate. The caller's process,
//! calibration, display and timing assertions are not authenticated here.
use crate::{Rect, Target, Window};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

const MAX_SCAN_TIME: Duration = Duration::from_secs(3);
pub const MAX_CALIBRATION_AGE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessIdentity {
    pub pid: i32,
    pub process_start_unix_seconds: f64,
    pub process_start_source: String,
    pub executable: Option<String>,
    pub bundle_id: Option<String>,
}

/// The display digest and calibration are caller assertions, not OS proof.
#[derive(Debug, Clone)]
pub struct CoordinateCalibration {
    pub display_sha256: String,
    pub calibrated_at: Instant,
    /// Nonempty identifier for the external calibration evidence.
    pub evidence_id: String,
}

#[derive(Debug, Clone)]
pub struct CgSnapshot {
    pub identity: ProcessIdentity,
    pub display_sha256: String,
    pub sampled_at: Instant,
    /// Caller reports an uninterrupted full inventory. This is not verified here.
    pub complete: bool,
    /// Complete global on-screen window list, front to back, including nonzero layers.
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone)]
pub struct AxWindow {
    pub owner_pid: Option<i32>,
    pub role: Option<String>,
    pub role_truncated: bool,
    pub title: Option<String>,
    pub title_truncated: bool,
    pub frame: Option<Rect>,
    /// Top-level AX index only. The index is ephemeral, not a persistent ID.
    pub path: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct AxSnapshot {
    pub identity: ProcessIdentity,
    pub display_sha256: String,
    pub started_at: Instant,
    pub completed_at: Instant,
    pub status_ok: bool,
    pub process_window_scope: bool,
    pub action_allowed: bool,
    pub truncated: bool,
    pub window_count_reported: usize,
    pub windows: Vec<AxWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchResult {
    Unknown(&'static str),
    Ambiguous(&'static str),
    CandidateUnverified { target: Target, ax_path: Vec<usize> },
}
impl MatchResult {
    /// No result permits an AX action, click, focus change or other input.
    pub const fn action_allowed(&self) -> bool {
        false
    }
}

fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn valid_identity(identity: &ProcessIdentity) -> bool {
    identity.pid > 0
        && identity.process_start_unix_seconds.is_finite()
        && identity.process_start_unix_seconds > 0.0
        && identity
            .executable
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty())
        && identity
            .bundle_id
            .as_deref()
            .is_none_or(|v| !v.trim().is_empty())
}
fn valid_frame(r: Rect) -> bool {
    r.valid() && (r.x + r.width).is_finite() && (r.y + r.height).is_finite()
}
fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}
fn valid_title(s: Option<&str>) -> bool {
    s.is_some_and(|s| !s.trim().is_empty())
}

/// Raw exact-title/frame diagnostic only. It is not a binding or an action target.
/// Completeness, display mapping, occlusion and observation provenance are not established.
pub fn unverified_geometry_hint(
    target: Target,
    before: &CgSnapshot,
    ax: &AxSnapshot,
    after: &CgSnapshot,
) -> Option<Vec<Vec<usize>>> {
    if Target::new(target.pid, target.window_id).is_err()
        || !valid_identity(&before.identity)
        || before.identity != after.identity
        || before.identity != ax.identity
        || before.identity.pid != target.pid
        || !ax.status_ok
        || !ax.process_window_scope
        || ax.action_allowed
        || ax.truncated
        || ax.window_count_reported != ax.windows.len()
        || ax.windows.len() > 256
        || ax.started_at < before.sampled_at
        || ax.completed_at < ax.started_at
        || ax.completed_at > after.sampled_at
        || after
            .sampled_at
            .checked_duration_since(before.sampled_at)
            .is_none_or(|d| d > MAX_SCAN_TIME)
    {
        return None;
    }
    let first: Vec<_> = before
        .windows
        .iter()
        .filter(|w| w.target == target)
        .collect();
    let last: Vec<_> = after
        .windows
        .iter()
        .filter(|w| w.target == target)
        .collect();
    if first.len() != 1
        || last.len() != 1
        || first[0] != last[0]
        || first[0].layer != Some(0)
        || !valid_frame(first[0].bounds)
        || !valid_title(first[0].title.as_deref())
    {
        return None;
    }
    if ax.windows.iter().enumerate().any(|(i, w)| {
        w.owner_pid != Some(target.pid)
            || w.role.as_deref() != Some("AXWindow")
            || w.role_truncated
            || w.title_truncated
            || !valid_title(w.title.as_deref())
            || !w.frame.is_some_and(valid_frame)
            || w.path != [i]
    }) {
        return None;
    }
    Some(
        ax.windows
            .iter()
            .filter(|w| w.frame == Some(first[0].bounds) && w.title == first[0].title)
            .map(|w| w.path.clone())
            .collect(),
    )
}

/// Compare a CG inventory, AX process snapshot and later CG inventory.
/// All inputs are caller assertions. Even a positive result is unverified.
pub fn match_candidate(
    target: Target,
    before: &CgSnapshot,
    ax: &AxSnapshot,
    after: &CgSnapshot,
    calibration: Option<&CoordinateCalibration>,
) -> MatchResult {
    let Some(calibration) = calibration else {
        return MatchResult::Unknown("calibration_missing");
    };
    if !sha(&calibration.display_sha256)
        || calibration.evidence_id.trim().is_empty()
        || calibration.evidence_id.len() > 200
    {
        return MatchResult::Unknown("calibration_invalid");
    }
    if before.display_sha256 != calibration.display_sha256
        || ax.display_sha256 != calibration.display_sha256
        || after.display_sha256 != calibration.display_sha256
    {
        return MatchResult::Unknown("display_changed_or_unverified");
    }
    let Some(calibration_age) = before
        .sampled_at
        .checked_duration_since(calibration.calibrated_at)
    else {
        return MatchResult::Unknown("calibration_time_invalid");
    };
    let Some(elapsed) = after.sampled_at.checked_duration_since(before.sampled_at) else {
        return MatchResult::Unknown("scan_time_invalid");
    };
    if calibration_age > MAX_CALIBRATION_AGE
        || elapsed > MAX_SCAN_TIME
        || ax.started_at < before.sampled_at
        || ax.completed_at < ax.started_at
        || ax.completed_at > after.sampled_at
    {
        return MatchResult::Unknown("scan_or_calibration_stale");
    }
    if Target::new(target.pid, target.window_id).is_err()
        || !valid_identity(&before.identity)
        || !valid_identity(&ax.identity)
        || !valid_identity(&after.identity)
        || before.identity != ax.identity
        || before.identity != after.identity
        || before.identity.pid != target.pid
    {
        return MatchResult::Unknown("process_identity_unverified_or_changed");
    }
    if !ax.status_ok
        || !ax.process_window_scope
        || ax.action_allowed
        || ax.truncated
        || ax.window_count_reported != ax.windows.len()
    {
        return MatchResult::Unknown("ax_snapshot_incomplete_or_unsafe");
    }
    if !before.complete || !after.complete {
        return MatchResult::Unknown("cg_snapshot_incomplete_or_invalid");
    }
    fn check_cg(snapshot: &CgSnapshot) -> bool {
        let mut ids = HashSet::new();
        snapshot.windows.iter().all(|w| {
            w.target.pid > 0
                && w.target.window_id > 0
                && ids.insert(w.target.window_id)
                && w.layer.is_some()
                && valid_frame(w.bounds)
        })
    }
    if !check_cg(before) || !check_cg(after) {
        return MatchResult::Unknown("cg_snapshot_incomplete_or_invalid");
    }
    // Order matters for occlusion; the caller must supply a full global list.
    if before.windows != after.windows {
        return MatchResult::Unknown("cg_window_set_changed");
    }
    let Some(target_index) = before.windows.iter().position(|w| w.target == target) else {
        return MatchResult::Unknown("target_missing");
    };
    let cg = &before.windows[target_index];
    if cg.layer != Some(0) {
        return MatchResult::Unknown("target_layer_not_normal");
    }
    if !valid_title(cg.title.as_deref()) {
        return MatchResult::Unknown("cg_target_title_missing");
    }
    for (index, w) in ax.windows.iter().enumerate() {
        if w.owner_pid != Some(target.pid)
            || w.role.as_deref() != Some("AXWindow")
            || w.role_truncated
            || w.title_truncated
            || !valid_title(w.title.as_deref())
            || !w.frame.is_some_and(valid_frame)
            || w.path != [index]
        {
            return MatchResult::Unknown("ax_window_incomplete_or_invalid");
        }
    }
    let candidates: Vec<_> = ax
        .windows
        .iter()
        .filter(|w| w.frame == Some(cg.bounds) && w.title == cg.title)
        .collect();
    if candidates.len() > 1 {
        return MatchResult::Ambiguous("multiple_exact_ax_candidates");
    }
    let Some(candidate) = candidates.first() else {
        return MatchResult::Unknown("no_exact_ax_candidate");
    };
    // Other overlapping surfaces may be occluding or aliasing the candidate.
    if before
        .windows
        .iter()
        .take(target_index)
        .any(|w| overlaps(w.bounds, cg.bounds))
        || ax
            .windows
            .iter()
            .any(|w| w.path != candidate.path && overlaps(w.frame.unwrap(), cg.bounds))
    {
        return MatchResult::Ambiguous("overlapping_window");
    }
    MatchResult::CandidateUnverified {
        target,
        ax_path: candidate.path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        target: Target,
        before: CgSnapshot,
        ax: AxSnapshot,
        after: CgSnapshot,
        calibration: CoordinateCalibration,
    }
    impl Fixture {
        fn new() -> Self {
            let t = Instant::now();
            let identity = ProcessIdentity {
                pid: 42,
                process_start_unix_seconds: 123.0,
                process_start_source: "kernel_sysctl_kern_proc".into(),
                executable: Some("/app/example".into()),
                bundle_id: Some("example.app".into()),
            };
            let target = Target::new(42, 7).unwrap();
            let bounds = Rect {
                x: -500.0,
                y: 20.0,
                width: 400.0,
                height: 300.0,
            };
            let window = Window {
                target,
                bounds,
                layer: Some(0),
                title: Some("Example".into()),
            };
            let before = CgSnapshot {
                identity: identity.clone(),
                display_sha256: "a".repeat(64),
                sampled_at: t + Duration::from_millis(100),
                complete: true,
                windows: vec![window],
            };
            let mut after = before.clone();
            after.sampled_at = t + Duration::from_millis(300);
            let ax = AxSnapshot {
                identity,
                display_sha256: "a".repeat(64),
                started_at: t + Duration::from_millis(150),
                completed_at: t + Duration::from_millis(200),
                status_ok: true,
                process_window_scope: true,
                action_allowed: false,
                truncated: false,
                window_count_reported: 1,
                windows: vec![AxWindow {
                    owner_pid: Some(42),
                    role: Some("AXWindow".into()),
                    role_truncated: false,
                    title: Some("Example".into()),
                    title_truncated: false,
                    frame: Some(bounds),
                    path: vec![0],
                }],
            };
            let calibration = CoordinateCalibration {
                display_sha256: "a".repeat(64),
                calibrated_at: t,
                evidence_id: "fixture-calibration".into(),
            };
            Self {
                target,
                before,
                ax,
                after,
                calibration,
            }
        }
        fn result(&self) -> MatchResult {
            match_candidate(
                self.target,
                &self.before,
                &self.ax,
                &self.after,
                Some(&self.calibration),
            )
        }
    }
    #[test]
    fn uncalibrated_geometry_hint_is_diagnostic_only() {
        let mut f = Fixture::new();
        assert_eq!(
            unverified_geometry_hint(f.target, &f.before, &f.ax, &f.after),
            Some(vec![vec![0]])
        );
        assert_eq!(
            match_candidate(f.target, &f.before, &f.ax, &f.after, None),
            MatchResult::Unknown("calibration_missing")
        );
        f.ax.windows[0].frame.as_mut().unwrap().x += 1.0;
        assert_eq!(
            unverified_geometry_hint(f.target, &f.before, &f.ax, &f.after),
            Some(vec![])
        );
        f.ax.truncated = true;
        assert_eq!(
            unverified_geometry_hint(f.target, &f.before, &f.ax, &f.after),
            None
        );
    }
    #[test]
    fn geometry_hint_rejects_changed_identity_and_target() {
        let mut f = Fixture::new();
        f.ax.identity.process_start_unix_seconds += 1.0;
        assert_eq!(
            unverified_geometry_hint(f.target, &f.before, &f.ax, &f.after),
            None
        );
        let mut f = Fixture::new();
        f.after.windows[0].bounds.x += 1.0;
        assert_eq!(
            unverified_geometry_hint(f.target, &f.before, &f.ax, &f.after),
            None
        );
        let mut f = Fixture::new();
        f.ax.windows.push(f.ax.windows[0].clone());
        f.ax.windows[1].path = vec![1];
        f.ax.window_count_reported = 2;
        assert_eq!(
            unverified_geometry_hint(f.target, &f.before, &f.ax, &f.after),
            Some(vec![vec![0], vec![1]])
        );
    }
    #[test]
    fn unique_match_is_only_unverified_candidate() {
        let f = Fixture::new();
        assert_eq!(
            f.result(),
            MatchResult::CandidateUnverified {
                target: f.target,
                ax_path: vec![0]
            }
        );
        assert!(!f.result().action_allowed());
    }
    #[test]
    fn rejects_absent_calibration_changed_display_and_stale_samples() {
        let mut f = Fixture::new();
        assert_eq!(
            match_candidate(f.target, &f.before, &f.ax, &f.after, None),
            MatchResult::Unknown("calibration_missing")
        );
        f.after.display_sha256 = "b".repeat(64);
        assert_eq!(
            f.result(),
            MatchResult::Unknown("display_changed_or_unverified")
        );
        f.after.display_sha256 = "a".repeat(64);
        f.after.sampled_at = f.before.sampled_at + Duration::from_secs(4);
        assert_eq!(
            f.result(),
            MatchResult::Unknown("scan_or_calibration_stale")
        );
        f.after.sampled_at = f.before.sampled_at - Duration::from_nanos(1);
        assert_eq!(f.result(), MatchResult::Unknown("scan_time_invalid"));
        f.after.sampled_at = f.before.sampled_at + Duration::from_millis(200);
        f.calibration.calibrated_at = f.before.sampled_at - Duration::from_secs(61);
        assert_eq!(
            f.result(),
            MatchResult::Unknown("scan_or_calibration_stale")
        );
    }
    #[test]
    fn rejects_missing_or_changed_identity() {
        let mut f = Fixture::new();
        f.ax.identity.process_start_unix_seconds = 124.0;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("process_identity_unverified_or_changed")
        );
        f.ax.identity = f.before.identity.clone();
        f.before.identity.executable = None;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("process_identity_unverified_or_changed")
        );
    }
    #[test]
    fn rejects_incomplete_ax_snapshot_and_fields() {
        let mut f = Fixture::new();
        f.ax.truncated = true;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_snapshot_incomplete_or_unsafe")
        );
        f.ax.truncated = false;
        f.ax.window_count_reported = 2;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_snapshot_incomplete_or_unsafe")
        );
        f.ax.window_count_reported = 1;
        f.ax.action_allowed = true;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_snapshot_incomplete_or_unsafe")
        );
        f.ax.action_allowed = false;
        f.ax.windows[0].owner_pid = None;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_window_incomplete_or_invalid")
        );
        f.ax.windows[0].owner_pid = Some(42);
        f.ax.windows[0].title_truncated = true;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_window_incomplete_or_invalid")
        );
        f.ax.windows[0].title_truncated = false;
        f.ax.windows[0].frame = None;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_window_incomplete_or_invalid")
        );
    }
    #[test]
    fn rejects_cg_missing_fields_nonzero_layer_and_set_change() {
        let mut f = Fixture::new();
        f.before.complete = false;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("cg_snapshot_incomplete_or_invalid")
        );
        f.before.complete = true;
        f.before.windows[0].layer = None;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("cg_snapshot_incomplete_or_invalid")
        );
        f.before.windows[0].layer = Some(1);
        f.after.windows[0].layer = Some(1);
        assert_eq!(f.result(), MatchResult::Unknown("target_layer_not_normal"));
        f.before.windows[0].layer = Some(0);
        f.after.windows[0].layer = Some(0);
        f.after.windows[0].bounds.x += 1.0;
        assert_eq!(f.result(), MatchResult::Unknown("cg_window_set_changed"));
        f.after.windows[0].bounds.x -= 1.0;
        f.before.windows[0].title = None;
        f.after.windows[0].title = None;
        assert_eq!(f.result(), MatchResult::Unknown("cg_target_title_missing"));
    }
    #[test]
    fn overlapping_or_duplicate_windows_are_ambiguous() {
        let mut f = Fixture::new();
        let mut second = f.ax.windows[0].clone();
        second.path = vec![1];
        f.ax.windows.push(second);
        f.ax.window_count_reported = 2;
        assert_eq!(
            f.result(),
            MatchResult::Ambiguous("multiple_exact_ax_candidates")
        );
        f.ax.windows[1].title = Some("Other".into());
        assert_eq!(f.result(), MatchResult::Ambiguous("overlapping_window"));
        f.ax.windows.pop();
        f.ax.window_count_reported = 1;
        let mut other = f.before.windows[0].clone();
        other.target = Target::new(99, 8).unwrap();
        other.title = Some("Overlay".into());
        other.layer = Some(2);
        f.before.windows.insert(0, other.clone());
        f.after.windows.insert(0, other);
        assert_eq!(f.result(), MatchResult::Ambiguous("overlapping_window"));
    }
    #[test]
    fn incomplete_ax_inputs_report_specific_unknown_reasons() {
        const UNSAFE: &str = "ax_snapshot_incomplete_or_unsafe";
        const INVALID: &str = "ax_window_incomplete_or_invalid";
        type Change = Box<dyn Fn(&mut Fixture)>;
        let cases: Vec<(&str, Change)> = vec![
            ("status", Box::new(|f| f.ax.status_ok = false)),
            ("scope", Box::new(|f| f.ax.process_window_scope = false)),
            (
                "role",
                Box::new(|f| f.ax.windows[0].role = Some("AXButton".into())),
            ),
            ("role none", Box::new(|f| f.ax.windows[0].role = None)),
            (
                "role truncated",
                Box::new(|f| f.ax.windows[0].role_truncated = true),
            ),
            (
                "blank title",
                Box::new(|f| f.ax.windows[0].title = Some("  ".into())),
            ),
            ("path", Box::new(|f| f.ax.windows[0].path = vec![1])),
        ];
        for (name, change) in cases {
            let mut f = Fixture::new();
            change(&mut f);
            let result = f.result();
            let expected = if matches!(name, "status" | "scope") {
                MatchResult::Unknown(UNSAFE)
            } else {
                MatchResult::Unknown(INVALID)
            };
            assert_eq!(result, expected, "{name}");
            assert!(!result.action_allowed(), "{name}");
        }
        let mut f = Fixture::new();
        f.before.windows[0].title = None;
        f.after.windows[0].title = None;
        let result = f.result();
        assert_eq!(result, MatchResult::Unknown("cg_target_title_missing"));
        assert!(!result.action_allowed());
    }
    #[test]
    fn duplicate_ids_and_bad_rectangles_fail_closed() {
        let mut f = Fixture::new();
        f.before.windows.push(f.before.windows[0].clone());
        assert_eq!(
            f.result(),
            MatchResult::Unknown("cg_snapshot_incomplete_or_invalid")
        );
        f.before.windows.pop();
        f.ax.windows[0].frame.as_mut().unwrap().x = f64::NAN;
        assert_eq!(
            f.result(),
            MatchResult::Unknown("ax_window_incomplete_or_invalid")
        );
    }
    #[test]
    fn distinct_windows_require_stable_front_to_back_order() {
        let mut f = Fixture::new();
        let mut other = f.before.windows[0].clone();
        other.target = Target::new(42, 8).unwrap();
        other.title = Some("Other".into());
        other.bounds.x = 1000.0;
        f.before.windows.push(other.clone());
        f.after.windows.push(other);
        let mut other_ax = f.ax.windows[0].clone();
        other_ax.title = Some("Other".into());
        other_ax.frame.as_mut().unwrap().x = 1000.0;
        other_ax.path = vec![1];
        f.ax.windows.push(other_ax);
        f.ax.window_count_reported = 2;
        assert_eq!(
            f.result(),
            MatchResult::CandidateUnverified {
                target: f.target,
                ax_path: vec![0]
            }
        );
        f.after.windows.reverse();
        assert_eq!(f.result(), MatchResult::Unknown("cg_window_set_changed"));
    }
    #[test]
    fn background_surface_does_not_count_as_foreground_occlusion() {
        let mut f = Fixture::new();
        let mut desktop = f.before.windows[0].clone();
        desktop.target = Target::new(99, 8).unwrap();
        desktop.layer = Some(-2147483603);
        desktop.title = None;
        f.before.windows.push(desktop.clone());
        f.after.windows.push(desktop);
        assert_eq!(
            f.result(),
            MatchResult::CandidateUnverified {
                target: f.target,
                ax_path: vec![0]
            }
        );
    }
}
