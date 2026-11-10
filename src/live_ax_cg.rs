//! Read-only collection for AX/CG analysis. No input or calibration is supplied.
use crate::{
    ax_cg::{self, AxSnapshot, AxWindow, CgSnapshot, MatchResult, ProcessIdentity},
    Backend, Rect, Target, Window,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn identity(v: &Value, pid: i32) -> Option<ProcessIdentity> {
    let start = v["process_start_unix_seconds"].as_f64()?;
    if v["process_start_source"] != "kernel_sysctl_kern_proc" {
        return None;
    }
    let executable = v["executable"].as_str()?.to_owned();
    if v["status"] != "ok"
        || v["action_allowed"] != false
        || v["pid"].as_i64()? != i64::from(pid)
        || !start.is_finite()
        || start <= 0.0
        || executable.trim().is_empty()
    {
        return None;
    }
    let bundle_id = match &v["bundle_id"] {
        Value::Null => None,
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => return None,
    };
    Some(ProcessIdentity {
        pid,
        process_start_unix_seconds: start,
        process_start_source: "kernel_sysctl_kern_proc".into(),
        executable: Some(executable),
        bundle_id,
    })
}
fn frame(v: &Value) -> Option<Rect> {
    Some(Rect {
        x: v["x"].as_f64()?,
        y: v["y"].as_f64()?,
        width: v["width"].as_f64()?,
        height: v["height"].as_f64()?,
    })
}
fn truncated(v: &Value, key: &str) -> bool {
    !matches!(v.get(key), None | Some(Value::Bool(false)))
}
fn path(v: &Value) -> Vec<usize> {
    v.as_array()
        .and_then(|a| {
            a.iter()
                .map(|n| n.as_u64().and_then(|x| usize::try_from(x).ok()))
                .collect()
        })
        .unwrap_or_default()
}
fn target_windows(windows: &[Window], target: Target) -> Vec<Value> {
    windows.iter().filter(|w| w.target == target).map(|w| json!({"window_id":w.target.window_id,"pid":w.target.pid,"layer":w.layer,"title":w.title,
        "bounds":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height}})).collect()
}
// These are raw, uncalibrated comparisons. Counts are diagnostics, not binding evidence.
fn ax_field_diagnostics(target: Target, cg: &[Window], windows: &[AxWindow]) -> Value {
    let matches: Vec<_> = cg
        .iter()
        .filter(|w| w.target == target && w.bounds.valid())
        .collect();
    let target_frame = if matches.len() == 1 {
        Some(matches[0].bounds)
    } else {
        None
    };
    let incomplete: Vec<_> = windows
        .iter()
        .filter(|w| w.title_truncated || w.title.as_deref().is_none_or(|s| s.trim().is_empty()))
        .collect();
    let unknown_frames = incomplete
        .iter()
        .filter(|w| !w.frame.is_some_and(|r| r.valid()))
        .count();
    let overlaps = if unknown_frames == 0 {
        target_frame.map(|a| {
            incomplete
                .iter()
                .filter(|w| {
                    w.frame.is_some_and(|b| {
                        b.valid()
                            && a.x < b.x + b.width
                            && b.x < a.x + a.width
                            && a.y < b.y + b.height
                            && b.y < a.y + a.height
                    })
                })
                .count()
        })
    } else {
        None
    };
    json!({"status":"uncalibrated_diagnostic", "action_allowed":false,
        "window_count":windows.len(), "missing_or_truncated_title_count":incomplete.len(),
        "incomplete_title_missing_frame_count":unknown_frames, "incomplete_title_raw_overlap_count":overlaps,
        "note":"Raw rectangles may use different coordinate systems; counts do not establish visibility, identity, or safe input."})
}

fn unix_millis() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis())
}
fn identity_evidence(value: &Option<ProcessIdentity>) -> Value {
    match value {
        Some(id) => {
            json!({"pid":id.pid,"process_start_unix_seconds":id.process_start_unix_seconds,"process_start_source":id.process_start_source,
            "executable":id.executable,"bundle_id":id.bundle_id})
        }
        None => Value::Null,
    }
}
fn valid_display_report(v: &Value) -> bool {
    if v["status"] != "observed"
        || v["action_allowed"] != false
        || v["coordinate_system"] != "core_graphics_global"
        || v["scope"] != "active_displays_only_online_mirrors_may_be_omitted"
        || v["coordinate_calibration"] != "unverified"
        || v["pixel_mapping"] != "unverified"
        || v["inventory"] != "two_matching_sequential_samples_not_atomic"
        || v["display_identity_persistent"] != "unverified"
    {
        return false;
    }
    let Some(main) = v["main_display_id"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= u32::MAX as u64)
    else {
        return false;
    };
    let Some(displays) = v["active_displays"]
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 32)
    else {
        return false;
    };
    let mut ids = HashSet::new();
    for display in displays {
        let Some(id) = display["display_id"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= u32::MAX as u64)
        else {
            return false;
        };
        let Some(mirror) = display.get("mirrors_display_id") else {
            return false;
        };
        if !ids.insert(id)
            || !(mirror.is_null()
                || mirror
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= u32::MAX as u64 && n != id))
            || display["pixels_wide"].as_u64().is_none_or(|n| n == 0)
            || display["pixels_high"].as_u64().is_none_or(|n| n == 0)
            || !display["rotation_degrees"]
                .as_f64()
                .is_some_and(f64::is_finite)
            || !frame(&display["bounds_cg_global"]).is_some_and(|r| {
                r.valid() && (r.x + r.width).is_finite() && (r.y + r.height).is_finite()
            })
        {
            return false;
        }
    }
    ids.contains(&main)
}
struct Evidence {
    started: Instant,
    started_unix_ms: Option<u128>,
    before: Option<ProcessIdentity>,
    ax: Option<ProcessIdentity>,
    after: Option<ProcessIdentity>,
    display_before: Option<Value>,
    display_after: Option<Value>,
    display_before_ms: Option<(u128, u128)>,
    display_after_ms: Option<(u128, u128)>,
    cg_before: Option<Value>,
    cg_after: Option<Value>,
    ax_windows: Option<Value>,
    lost_evidence: Vec<&'static str>,
}
impl Evidence {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            started_unix_ms: unix_millis(),
            before: None,
            ax: None,
            after: None,
            display_before: None,
            display_after: None,
            display_before_ms: None,
            display_after_ms: None,
            cg_before: None,
            cg_after: None,
            ax_windows: None,
            lost_evidence: Vec::new(),
        }
    }
    fn report(&self) -> Value {
        let total_elapsed_ms = self.started.elapsed().as_millis();
        json!({"started_unix_ms":self.started_unix_ms,"completed_unix_ms":unix_millis(),
            "total_elapsed_ms":total_elapsed_ms,"duration_clock":"monotonic",
            "wall_clock":"system_unverified",
            "display_before":self.display_before,"display_after":self.display_after,
            "display_before_interval_ms":self.display_before_ms,"display_after_interval_ms":self.display_after_ms,
            "cg_before":self.cg_before,"cg_after":self.cg_after,"ax_windows":self.ax_windows,
            "unavailable_partial_evidence":self.lost_evidence,
            "process_identity":{"before":identity_evidence(&self.before),
                "ax":identity_evidence(&self.ax),"after":identity_evidence(&self.after)}})
    }
}
fn failure(target: Target, stage: &str, error: &str, evidence: &Evidence) -> Value {
    json!({"schema_version":1,"status":"unknown","reason":"observation_failed","failed_stage":stage,
        "error":error,"pid":target.pid,"window_id":target.window_id,"action_allowed":false,"evidence":evidence.report()})
}
/// Samples identity, displays, global CG list, AX, CG list, displays, identity.
/// The missing display and coordinate calibration can never be supplied by a CLI flag.
/// No observation from this function authorizes input.
pub fn observe<B: Backend>(backend: &B, target: Target) -> Value {
    let mut evidence = Evidence::new();
    macro_rules! sample {
        ($stage:literal, $call:expr) => {
            match $call {
                Ok(v) => v,
                Err(e) => return failure(target, $stage, &e, &evidence),
            }
        };
    }
    let id_before_raw = sample!("identity_before", backend.identity(target.pid));
    evidence.before = identity(&id_before_raw, target.pid);
    let display_before_start = evidence.started.elapsed().as_millis();
    let display_before = backend.display_inventory();
    evidence.display_before_ms =
        Some((display_before_start, evidence.started.elapsed().as_millis()));
    let display_before = match display_before {
        Ok(v) => v,
        Err(e) => {
            evidence
                .lost_evidence
                .push("display_before_internal_partial_samples_not_returned");
            return failure(target, "display_before", &e, &evidence);
        }
    };
    evidence.display_before = Some(display_before.clone());
    let before_at = Instant::now();
    let before = sample!("cg_before", backend.windows(None));
    evidence.cg_before =
        Some(json!({"count":before.len(),"target":target_windows(&before,target)}));
    let ax_started = Instant::now();
    let ax_raw = sample!("accessibility", backend.accessibility_windows(target.pid));
    evidence.ax = identity(&ax_raw, target.pid);
    evidence.ax_windows = Some(json!({"status":ax_raw["status"],"scope":ax_raw["scope"],
        "truncated":ax_raw["truncated"],"tree_scope":ax_raw["tree_scope"],"window_count_reported":ax_raw["window_count_reported"],
        "cg_window_binding":ax_raw["cg_window_binding"],"action_allowed":ax_raw["action_allowed"],
        "windows":ax_raw["windows"].as_array().map(|windows| windows.iter().map(|w| json!({
            "owner_pid":w["owner_pid"],"path":w["path"],"role":w["role"],"role_truncated":w["role_truncated"],
            "title":w["title"],"title_truncated":w["title_truncated"],"frame":w["frame"]})).collect::<Vec<_>>())}));
    let ax_completed = Instant::now();
    let after = sample!("cg_after", backend.windows(None));
    evidence.cg_after = Some(json!({"count":after.len(),"target":target_windows(&after,target)}));
    let after_at = Instant::now();
    let display_after_start = evidence.started.elapsed().as_millis();
    let display_after = backend.display_inventory();
    evidence.display_after_ms = Some((display_after_start, evidence.started.elapsed().as_millis()));
    let display_after = match display_after {
        Ok(v) => v,
        Err(e) => {
            evidence
                .lost_evidence
                .push("display_after_internal_partial_samples_not_returned");
            return failure(target, "display_after", &e, &evidence);
        }
    };
    evidence.display_after = Some(display_after.clone());
    let id_after_raw = sample!("identity_after", backend.identity(target.pid));
    evidence.after = identity(&id_after_raw, target.pid);
    let mut findings = Vec::new();
    if !valid_display_report(&display_before) || !valid_display_report(&display_after) {
        findings.push("display_sample_invalid_or_unsupported");
    } else if ["main_display_id", "active_displays"]
        .iter()
        .any(|k| display_before[*k] != display_after[*k])
    {
        findings.push("display_sample_changed");
    } else {
        findings.push("display_samples_match_freshness_and_calibration_unverified");
    }
    let id_before = evidence.before.clone();
    let id_after = evidence.after.clone();
    let ax_identity = evidence.ax.clone();
    if id_before.is_none() || id_before != id_after || id_before != ax_identity {
        findings.push("process_identity_unverified_or_changed");
    }
    let empty_identity = ProcessIdentity {
        pid: target.pid,
        process_start_unix_seconds: 0.0,
        process_start_source: String::new(),
        executable: None,
        bundle_id: None,
    };
    let ax_windows = ax_raw["windows"].as_array().map(|items| {
        items
            .iter()
            .map(|v| AxWindow {
                owner_pid: v["owner_pid"].as_i64().and_then(|p| i32::try_from(p).ok()),
                role: v["role"].as_str().map(str::to_owned),
                role_truncated: truncated(v, "role_truncated"),
                title: v["title"].as_str().map(str::to_owned),
                title_truncated: truncated(v, "title_truncated"),
                frame: frame(&v["frame"]),
                path: path(&v["path"]),
            })
            .collect::<Vec<_>>()
    });
    let reported = ax_raw["window_count_reported"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok());
    if ax_raw["scope"] != "process_ax_windows"
        || ax_raw["cg_window_binding"] != "unverified"
        || ax_raw["truncated"] != false
        || ax_windows.is_none()
        || reported != ax_windows.as_ref().map(Vec::len)
    {
        findings.push("ax_snapshot_incomplete_or_unsafe");
    }
    let id = id_before.unwrap_or(empty_identity.clone());
    // A successfully parsed OS list does not independently prove global completeness.
    let cg_before = CgSnapshot {
        identity: id.clone(),
        display_sha256: String::new(),
        sampled_at: before_at,
        complete: false,
        windows: before.clone(),
    };
    let ax = AxSnapshot {
        identity: ax_identity.unwrap_or(empty_identity.clone()),
        display_sha256: String::new(),
        started_at: ax_started,
        completed_at: ax_completed,
        status_ok: ax_raw["status"] == "ok",
        process_window_scope: ax_raw["scope"] == "process_ax_windows",
        action_allowed: ax_raw["action_allowed"] != false,
        truncated: ax_raw["truncated"] != false,
        window_count_reported: reported.unwrap_or(usize::MAX),
        windows: ax_windows.unwrap_or_default(),
    };
    let cg_after = CgSnapshot {
        identity: id_after.unwrap_or(empty_identity),
        display_sha256: String::new(),
        sampled_at: after_at,
        complete: false,
        windows: after.clone(),
    };
    if ax.windows.iter().enumerate().any(|(i, w)| {
        w.owner_pid != Some(target.pid)
            || w.role.as_deref() != Some("AXWindow")
            || w.role_truncated
            || w.title_truncated
            || w.title.as_deref().is_none_or(|s| s.trim().is_empty())
            || !w.frame.is_some_and(|r| r.valid())
            || w.path != [i]
    }) {
        findings.push("ax_window_fields_unverified");
    }
    if before != after {
        findings.push("cg_window_set_changed");
    }
    if target_windows(&before, target).len() != 1 {
        findings.push("cg_target_missing_or_ambiguous");
    }
    findings.push("cg_inventory_completeness_unverified");
    findings.push("display_configuration_unverified");
    findings.push("coordinate_calibration_missing");
    let field_diagnostics = ax_field_diagnostics(target, &before, &ax.windows);
    let geometry_hint = match ax_cg::unverified_geometry_hint(target, &cg_before, &ax, &cg_after) {
        Some(paths) => {
            json!({"status":"raw_exact_matches", "candidate_count":paths.len(), "ax_paths":paths, "action_allowed":false,
            "note":"Not calibrated or verified; not a binding or an action target."})
        }
        None => {
            json!({"status":"unknown", "candidate_count":null, "ax_paths":[], "action_allowed":false})
        }
    };
    // The matcher deliberately receives no invented calibration.
    let result = ax_cg::match_candidate(target, &cg_before, &ax, &cg_after, None);
    let reason = match result {
        MatchResult::Unknown(r) | MatchResult::Ambiguous(r) => r,
        MatchResult::CandidateUnverified { .. } => "unexpected_candidate",
    };
    let ax_summaries: Vec<Value> = ax.windows.iter().map(|w| json!({"owner_pid":w.owner_pid,"role":w.role,"role_truncated":w.role_truncated,
        "title":w.title,"title_truncated":w.title_truncated,"path":w.path,"frame":w.frame.map(|r| json!({"x":r.x,"y":r.y,"width":r.width,"height":r.height}))})).collect();
    json!({"schema_version":1,"status":"unknown","reason":reason,"findings":findings,"pid":target.pid,"window_id":target.window_id,
        "action_allowed":false,"binding":"unverified","geometry_hint":geometry_hint,"ax_field_diagnostics":field_diagnostics,"elapsed_ms":after_at.duration_since(before_at).as_millis(),"evidence":evidence.report(),
        "cg_before_count":before.len(),"cg_after_count":after.len(),"cg_before_target":target_windows(&before,target),
        "cg_after_target":target_windows(&after,target),"ax_window_count_reported":reported,"ax_windows":ax_summaries})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, path::Path};
    struct Fake {
        calls: RefCell<Vec<&'static str>>,
        changed: bool,
        ax_fails: bool,
        identity_delay_ms: u64,
        display_mode: u8,
        ax_override: Option<Value>,
    }
    impl Backend for Fake {
        fn display_inventory(&self) -> Result<Value, String> {
            let mut calls = self.calls.borrow_mut();
            let second = calls.contains(&"display");
            calls.push("display");
            if self.display_mode == 3 || (self.display_mode == 2 && second) {
                return Err("display unavailable".into());
            }
            let mut value = json!({"status":"observed","action_allowed":false,"coordinate_system":"core_graphics_global",
                "scope":"active_displays_only_online_mirrors_may_be_omitted","coordinate_calibration":"unverified","pixel_mapping":"unverified",
                "inventory":"two_matching_sequential_samples_not_atomic","display_identity_persistent":"unverified",
                "main_display_id":1,"active_displays":[{"display_id":1,"bounds_cg_global":{"x":if self.display_mode == 1 && second {1} else {0},"y":0,"width":if self.display_mode == 4 {0} else {100},"height":80},"pixels_wide":200,"pixels_high":160,"rotation_degrees":0,"mirrors_display_id":null}]});
            if self.display_mode == 5 {
                value["active_displays"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("mirrors_display_id");
            }
            Ok(value)
        }
        fn identity(&self, pid: i32) -> Result<Value, String> {
            if self.identity_delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(self.identity_delay_ms));
            }
            let mut c = self.calls.borrow_mut();
            c.push("identity");
            Ok(
                json!({"status":"ok","action_allowed":false,"pid":pid,"process_start_source":"kernel_sysctl_kern_proc","process_start_unix_seconds":if self.changed && c.len()>1 { 124.0 } else {123.0},"executable":"/App","bundle_id":"test"}),
            )
        }
        fn windows(&self, pid: Option<i32>) -> Result<Vec<Window>, String> {
            assert!(pid.is_none());
            self.calls.borrow_mut().push("cg");
            Ok(vec![Window {
                target: Target::new(42, 7).unwrap(),
                bounds: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 80.0,
                },
                layer: Some(0),
                title: Some("Test".into()),
            }])
        }
        fn accessibility(&self, pid: i32) -> Result<Value, String> {
            self.calls.borrow_mut().push("ax");
            if self.ax_fails {
                return Err("unavailable".into());
            }
            if let Some(v) = &self.ax_override {
                return Ok(v.clone());
            }
            Ok(
                json!({"status":"ok","pid":pid,"process_start_source":"kernel_sysctl_kern_proc","process_start_unix_seconds":123.0,"executable":"/App","bundle_id":"test","scope":"process_ax_windows","cg_window_binding":"unverified","action_allowed":false,"truncated":false,"window_count_reported":1,"windows":[{"owner_pid":42,"path":[0],"role":"AXWindow","title":"Test","frame":{"x":0,"y":0,"width":100,"height":80}}]}),
            )
        }
        fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
            panic!("no capture or input")
        }
    }
    #[test]
    fn identity_requires_kernel_start_source() {
        let mut value = json!({"status":"ok","action_allowed":false,"pid":42,"process_start_unix_seconds":123.0,"process_start_source":"kernel_sysctl_kern_proc","executable":"/App","bundle_id":"test"});
        assert!(identity(&value, 42).is_some());
        value["process_start_source"] = "appkit_launch_date".into();
        assert!(identity(&value, 42).is_none());
        value
            .as_object_mut()
            .unwrap()
            .remove("process_start_source");
        assert!(identity(&value, 42).is_none());
    }
    #[test]
    fn incomplete_title_and_unknown_geometry_remain_diagnostic() {
        let target = Target::new(42, 7).unwrap();
        let window = Window {
            target,
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 80.0,
            },
            layer: Some(0),
            title: Some("Test".into()),
        };
        let missing = AxWindow {
            owner_pid: Some(42),
            role: Some("AXWindow".into()),
            role_truncated: false,
            title: None,
            title_truncated: false,
            frame: Some(Rect {
                x: 10.0,
                y: 10.0,
                width: 20.0,
                height: 20.0,
            }),
            path: vec![0],
        };
        let value = ax_field_diagnostics(
            target,
            std::slice::from_ref(&window),
            std::slice::from_ref(&missing),
        );
        assert_eq!(value["missing_or_truncated_title_count"], 1);
        assert_eq!(value["incomplete_title_raw_overlap_count"], 1);
        assert_eq!(value["action_allowed"], false);
        let mut unknown = missing;
        unknown.frame = None;
        let value = ax_field_diagnostics(target, std::slice::from_ref(&window), &[unknown]);
        assert_eq!(value["incomplete_title_missing_frame_count"], 1);
        assert!(value["incomplete_title_raw_overlap_count"].is_null());
        let value = ax_field_diagnostics(target, &[window.clone(), window], &[]);
        assert!(value["incomplete_title_raw_overlap_count"].is_null());
    }

    #[test]
    fn read_only_sequence_returns_unknown_even_for_matching_window() {
        let b = Fake {
            calls: RefCell::new(vec![]),
            changed: false,
            ax_fails: false,
            identity_delay_ms: 0,
            display_mode: 0,
            ax_override: None,
        };
        let v = observe(&b, Target::new(42, 7).unwrap());
        assert_eq!(
            *b.calls.borrow(),
            vec!["identity", "display", "cg", "ax", "cg", "display", "identity"]
        );
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["reason"], "calibration_missing");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["ax_windows"][0]["owner_pid"], 42);
        assert!(v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "display_samples_match_freshness_and_calibration_unverified"));
        assert!(v["evidence"]["display_before_interval_ms"].is_array());
    }
    #[test]
    fn reports_changed_identity_and_ax_failure_without_input() {
        let b = Fake {
            calls: RefCell::new(vec![]),
            changed: true,
            ax_fails: false,
            identity_delay_ms: 0,
            display_mode: 0,
            ax_override: None,
        };
        let v = observe(&b, Target::new(42, 7).unwrap());
        assert!(v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "process_identity_unverified_or_changed"));
        assert_eq!(
            v["evidence"]["process_identity"]["before"]["process_start_unix_seconds"],
            123.0
        );
        assert_eq!(
            v["evidence"]["process_identity"]["after"]["process_start_unix_seconds"],
            124.0
        );
        let b = Fake {
            calls: RefCell::new(vec![]),
            changed: false,
            ax_fails: true,
            identity_delay_ms: 0,
            display_mode: 0,
            ax_override: None,
        };
        let v = observe(&b, Target::new(42, 7).unwrap());
        assert_eq!(v["failed_stage"], "accessibility");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["evidence"]["process_identity"]["before"]["pid"], 42);
        assert!(v["evidence"]["process_identity"]["ax"].is_null());
        assert!(v["evidence"]["process_identity"]["after"].is_null());
        assert!(v["evidence"]["total_elapsed_ms"].as_u64().is_some());
        assert_eq!(*b.calls.borrow(), vec!["identity", "display", "cg", "ax"]);
        assert!(v["evidence"]["display_before"].is_object());
        assert!(v["evidence"]["display_after"].is_null());
        assert_eq!(v["evidence"]["cg_before"]["count"], 1);
        assert_eq!(
            v["evidence"]["cg_before"]["target"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn total_duration_includes_both_slow_identity_reads() {
        let b = Fake {
            calls: RefCell::new(vec![]),
            changed: false,
            ax_fails: false,
            identity_delay_ms: 30,
            display_mode: 0,
            ax_override: None,
        };
        let v = observe(&b, Target::new(42, 7).unwrap());
        let total = v["evidence"]["total_elapsed_ms"].as_u64().unwrap();
        let scan = v["elapsed_ms"].as_u64().unwrap();
        assert!(
            total >= scan + 50,
            "identity reads must be included in total duration"
        );
        assert_eq!(v["evidence"]["duration_clock"], "monotonic");
        assert_eq!(v["evidence"]["wall_clock"], "system_unverified");
        assert!(v["evidence"]["started_unix_ms"].as_u64().is_some());
        assert!(v["evidence"]["completed_unix_ms"].as_u64().is_some());
        assert_eq!(
            v["evidence"]["process_identity"]["ax"]["executable"],
            "/App"
        );
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["action_allowed"], false);
    }
    #[test]
    fn changed_or_missing_display_never_grants_input_and_keeps_evidence() {
        for mode in [1, 2, 3, 4, 5] {
            let b = Fake {
                calls: RefCell::new(vec![]),
                changed: false,
                ax_fails: false,
                identity_delay_ms: 0,
                display_mode: mode,
                ax_override: None,
            };
            let v = observe(&b, Target::new(42, 7).unwrap());
            assert_eq!(v["status"], "unknown");
            assert_eq!(v["action_allowed"], false);
            if mode == 1 {
                assert!(v["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x == "display_sample_changed"));
            } else if mode == 4 || mode == 5 {
                assert!(v["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x == "display_sample_invalid_or_unsupported"));
            } else {
                assert_eq!(
                    v["failed_stage"],
                    if mode == 2 {
                        "display_after"
                    } else {
                        "display_before"
                    }
                );
                assert_eq!(v["evidence"]["display_before"].is_object(), mode == 2);
                if mode == 2 {
                    assert!(v["evidence"]["cg_before"].is_object());
                    assert!(v["evidence"]["cg_after"].is_object());
                    assert_eq!(v["evidence"]["ax_windows"]["scope"], "process_ax_windows");
                    assert_eq!(v["evidence"]["ax_windows"]["status"], "ok");
                    assert_eq!(v["evidence"]["ax_windows"]["action_allowed"], false);
                    assert_eq!(v["evidence"]["ax_windows"]["truncated"], false);
                    assert_eq!(v["evidence"]["ax_windows"]["window_count_reported"], 1);
                    assert_eq!(
                        v["evidence"]["ax_windows"]["cg_window_binding"],
                        "unverified"
                    );
                    assert!(v["evidence"]["ax_windows"]["windows"].is_array());
                    assert!(v["evidence"]["unavailable_partial_evidence"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|x| x == "display_after_internal_partial_samples_not_returned"));
                }
                assert!(v["evidence"][if mode == 2 {
                    "display_after_interval_ms"
                } else {
                    "display_before_interval_ms"
                }]
                .is_array());
            }
        }
    }

    fn good_ax() -> Value {
        json!({"status":"ok","pid":42,"process_start_source":"kernel_sysctl_kern_proc","process_start_unix_seconds":123.0,
            "executable":"/App","bundle_id":"test","scope":"process_ax_windows","cg_window_binding":"unverified",
            "action_allowed":false,"truncated":false,"window_count_reported":1,
            "windows":[{"owner_pid":42,"path":[0],"role":"AXWindow","title":"Test","frame":{"x":0,"y":0,"width":100,"height":80}}]})
    }
    fn observe_with_ax(ax: Value) -> Value {
        let b = Fake {
            calls: RefCell::new(vec![]),
            changed: false,
            ax_fails: false,
            identity_delay_ms: 0,
            display_mode: 0,
            ax_override: Some(ax),
        };
        observe(&b, Target::new(42, 7).unwrap())
    }
    /// Every incomplete AX result stays unknown, never authorizes input and keeps CG evidence.
    fn assert_unknown_and_safe(v: &Value) {
        assert_eq!(v["status"], "unknown");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["binding"], "unverified");
        assert_eq!(v["geometry_hint"]["action_allowed"], false);
        assert_eq!(v["cg_before_target"].as_array().unwrap().len(), 1);
        assert_eq!(v["evidence"]["process_identity"]["before"]["pid"], 42);
        for forbidden in ["ax_path", "click", "target_point"] {
            assert!(
                v.get(forbidden).is_none(),
                "{forbidden} must not be present"
            );
        }
    }
    fn has_finding(v: &Value, name: &str) -> bool {
        v["findings"].as_array().unwrap().iter().any(|x| x == name)
    }

    #[test]
    fn ax_truncated_keeps_windows_and_flags_unsafe() {
        let mut ax = good_ax();
        ax["truncated"] = true.into();
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "ax_snapshot_incomplete_or_unsafe"));
        assert_eq!(v["ax_windows"].as_array().unwrap().len(), 1);
        assert_eq!(v["ax_windows"][0]["title"], "Test");
        assert_eq!(v["evidence"]["ax_windows"]["truncated"], true);
        assert_eq!(v["geometry_hint"]["status"], "unknown");
    }
    #[test]
    fn ax_error_status_without_windows_keeps_other_evidence() {
        let ax = json!({"status":"error","scope":"process_ax_windows","truncated":false,
            "action_allowed":false,"window_count_reported":0});
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "ax_snapshot_incomplete_or_unsafe"));
        assert!(has_finding(&v, "process_identity_unverified_or_changed"));
        assert!(v["ax_windows"].as_array().unwrap().is_empty());
        assert_eq!(v["evidence"]["cg_before"]["count"], 1);
        assert_eq!(v["evidence"]["ax_windows"]["status"], "error");
    }
    #[test]
    fn ax_count_mismatch_is_flagged() {
        let mut ax = good_ax();
        ax["window_count_reported"] = 2.into();
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "ax_snapshot_incomplete_or_unsafe"));
        assert_eq!(v["ax_window_count_reported"], 2);
        assert_eq!(v["ax_windows"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn ax_wrong_scope_is_flagged() {
        let mut ax = good_ax();
        ax["scope"] = "application_tree".into();
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "ax_snapshot_incomplete_or_unsafe"));
    }
    #[test]
    fn ax_missing_title_frame_role_keep_remaining_fields() {
        let frame = json!({"x":0,"y":0,"width":100,"height":80});
        let cases = [
            (
                "missing title",
                json!({"owner_pid":42,"path":[0],"role":"AXWindow","frame":frame}),
            ),
            (
                "null frame",
                json!({"owner_pid":42,"path":[0],"role":"AXWindow","title":"B","frame":null}),
            ),
            (
                "wrong role",
                json!({"owner_pid":42,"path":[0],"role":"AXButton","title":"C","frame":frame}),
            ),
        ];
        for (name, window) in cases {
            let mut ax = good_ax();
            ax["windows"] = json!([window.clone()]);
            let v = observe_with_ax(ax);
            assert_unknown_and_safe(&v);
            assert!(has_finding(&v, "ax_window_fields_unverified"), "{name}");
            assert_eq!(v["ax_windows"].as_array().unwrap().len(), 1, "{name}");
            assert_eq!(v["ax_windows"][0]["title"], window["title"], "{name}");
            assert_eq!(v["ax_windows"][0]["role"], window["role"], "{name}");
            assert_eq!(
                v["ax_field_diagnostics"]["status"], "uncalibrated_diagnostic",
                "{name}"
            );
            let missing_title = i64::from(name == "missing title");
            assert_eq!(
                v["ax_field_diagnostics"]["missing_or_truncated_title_count"], missing_title,
                "{name}"
            );
        }
    }
    #[test]
    fn ax_truncated_title_and_role_flags_are_preserved() {
        let mut ax = good_ax();
        ax["windows"][0]["title_truncated"] = true.into();
        ax["windows"][0]["role_truncated"] = true.into();
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "ax_window_fields_unverified"));
        assert_eq!(v["ax_windows"][0]["title_truncated"], true);
        assert_eq!(v["ax_windows"][0]["role_truncated"], true);
    }
    #[test]
    fn duplicate_ax_windows_are_listed_not_resolved() {
        let mut ax = good_ax();
        ax["window_count_reported"] = 2.into();
        ax["windows"] = json!([
            {"owner_pid":42,"path":[0],"role":"AXWindow","title":"Test","frame":{"x":0,"y":0,"width":100,"height":80}},
            {"owner_pid":42,"path":[1],"role":"AXWindow","title":"Test","frame":{"x":0,"y":0,"width":100,"height":80}}
        ]);
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert_eq!(v["geometry_hint"]["status"], "raw_exact_matches");
        assert_eq!(v["geometry_hint"]["candidate_count"], 2);
        assert_eq!(v["geometry_hint"]["ax_paths"], json!([[0], [1]]));
        assert_eq!(v["geometry_hint"]["action_allowed"], false);
    }
    #[test]
    fn ax_error_status_with_windows_is_flagged_as_identity_problem() {
        let mut ax = good_ax();
        ax["status"] = "error".into();
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "process_identity_unverified_or_changed"));
        assert_eq!(v["ax_windows"].as_array().unwrap().len(), 1);
        assert_eq!(v["evidence"]["ax_windows"]["status"], "error");
    }
    #[test]
    fn ax_window_with_missing_owner_pid_is_flagged() {
        let mut ax = good_ax();
        ax["windows"][0]
            .as_object_mut()
            .unwrap()
            .remove("owner_pid");
        let v = observe_with_ax(ax);
        assert_unknown_and_safe(&v);
        assert!(has_finding(&v, "ax_window_fields_unverified"));
        assert_eq!(v["ax_windows"][0]["title"], "Test");
    }
}
