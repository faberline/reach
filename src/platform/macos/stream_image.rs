//! Capture a single stream sample and its metadata. No input is authorized.
use crate::{
    stream_status::{valid_identity, validate_sample_scope, window_json},
    Backend, Target,
};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    process::Command,
    time::Duration,
};

#[repr(C)]
struct MachTimebase {
    numer: u32,
    denom: u32,
}
unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebase) -> i32;
}

fn clock_ages_ms(
    sample: u64,
    receipt: u64,
    now: u64,
    numer: u32,
    denom: u32,
) -> Option<(f64, f64)> {
    if sample == 0 || sample > receipt || receipt > now || numer == 0 || denom == 0 {
        return None;
    }
    let ms = |ticks: u64| ((ticks as u128 * numer as u128) as f64) / (denom as f64) / 1_000_000.0;
    Some((ms(receipt - sample), ms(now - sample)))
}
fn age_at_record(first: &Value, receipt_age: f64) -> Option<f64> {
    let sample = first["sample_host_ticks"].as_u64()?;
    let receipt = first["receipt_host_ticks"].as_u64()?;
    let mut base = MachTimebase { numer: 0, denom: 0 };
    if unsafe { mach_timebase_info(&mut base) } != 0 {
        return None;
    }
    let now = unsafe { mach_absolute_time() };
    let (calculated, at_record) = clock_ages_ms(sample, receipt, now, base.numer, base.denom)?;
    // Detect mismatched clock domains or malformed helper data; never repair it.
    if (calculated - receipt_age).abs() > 1.0 {
        return None;
    }
    Some(at_record)
}

/// A process-local diagnostic interval. Start it before capture and never persist
/// or reuse it. It cannot prove screen freshness or authorize input.
pub struct LiveFrameClock {
    started: u64,
    numer: u32,
    denom: u32,
}
impl LiveFrameClock {
    pub fn start() -> Option<Self> {
        let mut base = MachTimebase { numer: 0, denom: 0 };
        if unsafe { mach_timebase_info(&mut base) } != 0 || base.numer == 0 || base.denom == 0 {
            return None;
        }
        Some(Self {
            started: unsafe { mach_absolute_time() },
            numer: base.numer,
            denom: base.denom,
        })
    }
    pub fn age_at_handoff_ms(&self, reference: &Value) -> Option<f64> {
        if reference["scope"] != "same_stream_sample_host_clock_v1" {
            return None;
        }
        let sample = reference["sample_host_ticks"].as_u64()?;
        let receipt = reference["receipt_host_ticks"].as_u64()?;
        let reported = reference["sample_age_at_receipt_ms"]
            .as_f64()
            .filter(|n| n.is_finite() && *n >= 0.0)?;
        live_age_ms(
            self.started,
            sample,
            receipt,
            unsafe { mach_absolute_time() },
            self.numer,
            self.denom,
            reported,
        )
    }
}
fn live_age_ms(
    start: u64,
    sample: u64,
    receipt: u64,
    now: u64,
    numer: u32,
    denom: u32,
    reported: f64,
) -> Option<f64> {
    if sample < start || !reported.is_finite() || reported < 0.0 {
        return None;
    }
    let (at_receipt, at_handoff) = clock_ages_ms(sample, receipt, now, numer, denom)?;
    if (at_receipt - reported).abs() > 1.0 {
        return None;
    }
    Some(at_handoff)
}

fn check_new_image(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.extension().and_then(|v| v.to_str()) != Some("png") {
        return Err("image must be an absolute PNG path".into());
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err("image already exists".into()),
        Err(e) => Err(format!("cannot inspect image path: {e}")),
    }
}
fn frame_matches(frame: &Value, w: &crate::Window) -> bool {
    [
        ("x", w.bounds.x),
        ("y", w.bounds.y),
        ("width", w.bounds.width),
        ("height", w.bounds.height),
    ]
    .iter()
    .all(|(key, value)| frame[*key].as_f64() == Some(*value))
}
fn geometry_diagnostic(sample: &Value) -> Value {
    let unknown = || {
        json!({"status":"unavailable","action_allowed":false,
        "note":"Missing or invalid geometry; image coordinates and input remain unverified."})
    };
    let first = &sample["first"];
    let positive = |v: &Value| v.as_f64().filter(|n| n.is_finite() && *n > 0.0);
    let Some(content_scale) = positive(&first["content_scale"]) else {
        return unknown();
    };
    let Some(scale_factor) = positive(&first["scale_factor"]) else {
        return unknown();
    };
    let Some(filter_scale) = positive(&sample["filter_point_pixel_scale"]) else {
        return unknown();
    };
    let valid_rect = |v: &Value| {
        ["x", "y"]
            .iter()
            .all(|k| v[*k].as_f64().is_some_and(f64::is_finite))
            && ["width", "height"]
                .iter()
                .all(|k| positive(&v[*k]).is_some())
    };
    if first["status"] != 0
        || !valid_rect(&first["content_rect_points"])
        || !valid_rect(&sample["filter_content_rect_points"])
    {
        return unknown();
    }
    json!({"status":"reported_unverified","action_allowed":false,"content_scale":content_scale,
       "content_scale_exactly_one":content_scale==1.0,"scale_factor":scale_factor,"filter_point_pixel_scale":filter_scale,
       "content_rect_points":first["content_rect_points"],"filter_content_rect_points":sample["filter_content_rect_points"],
       "note":"Reported metadata is not coordinate calibration or input authority; a scale of one does not prove identical geometry."})
}
fn timing_diagnostic(sample: &Value) -> Value {
    let first = &sample["first"];
    let age = first["sample_age_at_receipt_ms"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0);
    if first["status"] != 0 || first["stream_clock_available"] != true || age.is_none() {
        return json!({"status":"unavailable","action_allowed":false,
            "note":"Missing, invalid or non-complete timing; no freshness or input authority."});
    }
    let record_age = age.and_then(|a| age_at_record(first, a));
    json!({"status":"reported_unverified","action_allowed":false,"sample_age_at_receipt_ms":age,
        "sample_age_at_record_ms":record_age,"record_age_status":if record_age.is_some(){"reported_unverified"}else{"unavailable"},
        "scope":"same_frame_at_helper_callback_and_rust_record","note":"Ages are observations at callback and record, not at later use. They do not prove app content updated, identity, or input safety."})
}
/// Trusted helper writes image exclusively. Any failure preserves evidence and does not claim image ownership.
pub fn record<B: Backend>(
    backend: &B,
    target: Target,
    helper: &Path,
    image: &Path,
    evidence: &Path,
) -> Result<Value, String> {
    Target::new(target.pid, target.window_id)?;
    if !helper.is_absolute() || !evidence.is_absolute() || image == evidence {
        return Err("invalid helper or evidence path".into());
    }
    check_new_image(image)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(evidence)
        .map_err(|e| format!("cannot reserve evidence: {e}"))?;
    let mut r = json!({"schema":"cu-stream-image-evidence-v1","status":"pending","action_allowed":false,
      "target":{"pid":target.pid,"window_id":target.window_id},"requested_image_path":image,"image_ownership":"unverified",
      "sample":null,"geometry_diagnostic":null,"timing_diagnostic":null,"identity_before":null,"identity_after":null,"window_before":null,"window_after":null,"error":null,
      "note":"Same-sample status and image; coordinate calibration, content freshness, continuous identity and input isolation are unverified."});
    // Preserve an explicit unknown outcome before touching the target or helper.
    let pending = serde_json::to_vec_pretty(&r).map_err(|e| e.to_string())?;
    file.write_all(&pending)
        .and_then(|_| file.sync_all())
        .map_err(|e| {
            format!(
                "cannot persist pending evidence at {}: {e}",
                evidence.display()
            )
        })?;
    let result = (|| -> Result<(String, usize, usize), String> {
        let _lock = crate::capture_lock::acquire()?;
        let before = backend.identity(target.pid)?;
        r["identity_before"] = before.clone();
        if !valid_identity(&before, target.pid) {
            return Err("initial process identity invalid".into());
        }
        let w1 = backend.exact_window(target)?;
        r["window_before"] = window_json(&w1);
        if w1.target != target || w1.layer != Some(0) || !w1.bounds.valid() {
            return Err("initial window invalid".into());
        }
        let out = crate::bounded_process::run(
            Command::new(helper)
                .arg(target.pid.to_string())
                .arg(target.window_id.to_string())
                .arg(image),
            Duration::from_secs(30),
        )?;
        if !out.status.success() {
            return Err(format!(
                "stream image helper failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let sample: Value = crate::strict_json::from_slice(&out.stdout)
            .map_err(|e| format!("invalid stream image JSON: {e}"))?;
        r["sample"] = sample.clone();
        let complete = validate_sample_scope(&sample, target, "stream_image_same_sample")?;
        if !complete || sample["outcome"] != "captured" {
            return Err("stream frame is not a complete image".into());
        }
        if sample["image_path"].as_str() != image.to_str()
            || !frame_matches(&sample["window_frame_points"], &w1)
        {
            return Err("stream image target or frame mismatched".into());
        }
        let w2 = backend.exact_window(target)?;
        r["window_after"] = window_json(&w2);
        let after = backend.identity(target.pid)?;
        r["identity_after"] = after.clone();
        if !valid_identity(&after, target.pid)
            || before != after
            || w1.target != w2.target
            || w1.layer != w2.layer
            || w1.bounds != w2.bounds
        {
            return Err("process or window changed during capture".into());
        }
        let (digest, width, height) = crate::image_info(image)?;
        if sample["image_width"].as_u64() != Some(width as u64)
            || sample["image_height"].as_u64() != Some(height as u64)
        {
            return Err("stream image dimensions mismatch".into());
        }
        r["geometry_diagnostic"] = geometry_diagnostic(&sample);
        r["timing_diagnostic"] = timing_diagnostic(&sample);
        Ok((digest, width, height))
    })();
    match result {
        Ok((digest, width, height)) => {
            r["status"] = "captured".into();
            r["image_ownership"] = "helper_created_unverified".into();
            r["image_sha256"] = digest.into();
            r["image_width"] = width.into();
            r["image_height"] = height.into();
        }
        Err(error) => {
            r["status"] = "error".into();
            r["error"] = error.into();
        }
    }
    let bytes = serde_json::to_vec_pretty(&r).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&bytes))
        .and_then(|_| file.set_len(bytes.len() as u64))
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("evidence may be partial at {}: {e}", evidence.display()))?;
    Ok(r)
}

/// Adapts same-sample capture to the existing read-only observation pipeline.
/// The stream sidecar is separate from the outer capture sidecar.
pub struct StreamImageView<'a, B: Backend> {
    backend: &'a B,
    target: Target,
    helper: &'a Path,
}
impl<'a, B: Backend> StreamImageView<'a, B> {
    pub fn new(backend: &'a B, target: Target, helper: &'a Path) -> Result<Self, String> {
        Target::new(target.pid, target.window_id)?;
        if !helper.is_absolute() {
            return Err("stream helper must be absolute".into());
        }
        Ok(Self {
            backend,
            target,
            helper,
        })
    }
}
impl<B: Backend> Backend for StreamImageView<'_, B> {
    fn inventory_scope(&self) -> &'static str {
        "exact_window_candidate"
    }
    fn identity(&self, pid: i32) -> Result<Value, String> {
        if pid != self.target.pid {
            return Err("target PID mismatch".into());
        }
        self.backend.identity(pid)
    }
    fn windows(&self, pid: Option<i32>) -> Result<Vec<crate::Window>, String> {
        if pid != Some(self.target.pid) {
            return Err("target PID mismatch".into());
        }
        Ok(vec![self.backend.exact_window(self.target)?])
    }
    fn capture(&self, target: Target, image: &Path) -> Result<Value, String> {
        if target != self.target {
            return Err("capture target mismatch".into());
        }
        let mut name = image
            .file_name()
            .ok_or("image filename missing")?
            .to_os_string();
        name.push(".stream.json");
        let evidence = image.with_file_name(name);
        let r = record(self.backend, target, self.helper, image, &evidence)?;
        if r["status"] != "captured" || r["action_allowed"] != false {
            return Err(format!(
                "stream capture incomplete; evidence at {}: {}",
                evidence.display(),
                r["error"].as_str().unwrap_or("unknown")
            ));
        }
        Ok(
            json!({"window_id":target.window_id,"owner_pid":target.pid,"image_path":image,
          "window_frame_points":r["sample"]["window_frame_points"],"image_width":r["image_width"],"image_height":r["image_height"],
          "image_sha256":r["image_sha256"],"capture_type":"same_sample_stream","stream_evidence_file":evidence,
          "stream_frame_status":r["sample"]["first"]["status"],"geometry_diagnostic":r["geometry_diagnostic"],
          "timing_diagnostic":r["timing_diagnostic"],"timing_reference":{
            "scope":"same_stream_sample_host_clock_v1","sample_host_ticks":r["sample"]["first"]["sample_host_ticks"],
            "receipt_host_ticks":r["sample"]["first"]["receipt_host_ticks"],"sample_age_at_receipt_ms":r["sample"]["first"]["sample_age_at_receipt_ms"]},
          "coordinate_calibration":"unverified"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, os::unix::fs::PermissionsExt};
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60,
        0x60, 0x60, 0xf8, 0x0f, 0x00, 0x01, 0x04, 0x01, 0x00, 0x5f, 0xe5, 0xc3, 0x4b, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    struct Fake {
        calls: Cell<u32>,
        change: bool,
    }
    #[test]
    fn host_clock_conversion_refuses_reversal_and_invalid_timebase() {
        assert_eq!(
            clock_ages_ms(1_000_000, 2_000_000, 3_000_000, 1, 1),
            Some((1.0, 2.0))
        );
        assert_eq!(clock_ages_ms(2, 1, 3, 1, 1), None);
        assert_eq!(clock_ages_ms(1, 3, 2, 1, 1), None);
        assert_eq!(clock_ages_ms(0, 1, 2, 1, 1), None);
        assert_eq!(clock_ages_ms(1, 2, 3, 1, 0), None);
        assert_eq!(
            live_age_ms(500_000, 1_000_000, 2_000_000, 3_000_000, 1, 1, 1.0),
            Some(2.0)
        );
        assert_eq!(
            live_age_ms(1_500_000, 1_000_000, 2_000_000, 3_000_000, 1, 1, 1.0),
            None
        );
        assert_eq!(
            live_age_ms(500_000, 1_000_000, 2_000_000, 3_000_000, 1, 1, 5.0),
            None
        );
    }
    #[test]
    fn timing_never_grants_authority_and_invalid_data_stays_unknown() {
        let mut s = json!({"first":{"status":0,"stream_clock_available":true,"sample_age_at_receipt_ms":6.2}});
        let v = timing_diagnostic(&s);
        assert_eq!(v["status"], "reported_unverified");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["record_age_status"], "unavailable");
        assert!(v["sample_age_at_record_ms"].is_null());
        s["first"]["sample_age_at_receipt_ms"] = (-1).into();
        assert_eq!(timing_diagnostic(&s)["status"], "unavailable");
        s["first"]["sample_age_at_receipt_ms"] = 6.into();
        s["first"]["stream_clock_available"] = false.into();
        assert_eq!(timing_diagnostic(&s)["status"], "unavailable");
        s["first"]["stream_clock_available"] = true.into();
        s["first"]["status"] = 3.into();
        assert_eq!(timing_diagnostic(&s)["status"], "unavailable");
    }
    #[test]
    fn geometry_is_diagnostic_and_missing_or_invalid_values_are_unknown() {
        let mut s = json!({"first":{"status":0,"content_scale":0.958656,"scale_factor":2,"content_rect_points":{"x":0,"y":0,"width":1056,"height":742}},
      "filter_point_pixel_scale":2,"filter_content_rect_points":{"x":-1102,"y":0,"width":1102,"height":742}});
        let v = geometry_diagnostic(&s);
        assert_eq!(v["status"], "reported_unverified");
        assert_eq!(v["content_scale_exactly_one"], false);
        assert_eq!(v["action_allowed"], false);
        s["first"]["content_scale"] = 1.into();
        assert_eq!(geometry_diagnostic(&s)["content_scale_exactly_one"], true);
        s["first"]["content_scale"] = 0.into();
        assert_eq!(geometry_diagnostic(&s)["status"], "unavailable");
        s["first"]["content_scale"] = 1.into();
        s["first"]["content_rect_points"]["width"] = "bad".into();
        assert_eq!(geometry_diagnostic(&s)["status"], "unavailable");
        s["first"]["content_rect_points"]["width"] = 1056.into();
        s["first"]["status"] = 3.into();
        assert_eq!(geometry_diagnostic(&s)["status"], "unavailable");
    }
    impl Backend for Fake {
        fn identity(&self, pid: i32) -> Result<Value, String> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            Ok(
                json!({"status":"ok","action_allowed":false,"pid":pid,"process_start_source":"kernel_sysctl_kern_proc","process_start_unix_seconds":if self.change && n>0 {124} else {123},"executable":"/test"}),
            )
        }
        fn windows(&self, _: Option<i32>) -> Result<Vec<crate::Window>, String> {
            Err("unused".into())
        }
        fn exact_window(&self, target: Target) -> Result<crate::Window, String> {
            Ok(crate::Window {
                target,
                bounds: crate::Rect {
                    x: 0.,
                    y: 0.,
                    width: 100.,
                    height: 80.,
                },
                layer: Some(0),
                title: None,
            })
        }
        fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
            Err("unused".into())
        }
    }
    #[test]
    fn pending_evidence_exists_before_backend_observation() {
        struct Inspect {
            evidence: std::path::PathBuf,
        }
        impl Backend for Inspect {
            fn identity(&self, _: i32) -> Result<Value, String> {
                let v: Value = serde_json::from_slice(&fs::read(&self.evidence).unwrap()).unwrap();
                assert_eq!(v["status"], "pending");
                assert_eq!(v["action_allowed"], false);
                assert_eq!(v["image_ownership"], "unverified");
                Err("intentional observation failure".into())
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<crate::Window>, String> {
                Err("unused".into())
            }
            fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
                Err("unused".into())
            }
        }
        let _guard = crate::TEST_CAPTURE_MUTEX.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("cu-stream-pending-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let evidence = dir.join("evidence.json");
        let image = dir.join("image.png");
        let result = record(
            &Inspect {
                evidence: evidence.clone(),
            },
            Target::new(42, 99).unwrap(),
            Path::new("/nonexistent-helper"),
            &image,
            &evidence,
        )
        .unwrap();
        assert_eq!(result["status"], "error");
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&evidence).unwrap()).unwrap(),
            result
        );
        assert!(!image.exists());
        fs::remove_file(evidence).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn captures_binds_and_refuses_changed_identity_or_existing_evidence() {
        let _guard = crate::TEST_CAPTURE_MUTEX.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("cu-stream-image-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        let src = dir.join("source.png");
        fs::write(&src, PNG).unwrap();
        let target = Target::new(42, 99).unwrap();
        let backend = Fake {
            calls: Cell::new(0),
            change: false,
        };
        let image = dir.join("ok.png");
        let evidence = dir.join("ok.json");
        let sample = |path: &Path| {
            json!({"schema":"cu-stream-status-v1","scope":"stream_image_same_sample","pid":42,"window_id":99,"input_posted":false,"action_allowed":false,
     "onscreen_before":true,"onscreen_after":true,"received":true,"outcome":"captured","image_path":path,"image_width":1,"image_height":1,
     "window_frame_points":{"x":0.,"y":0.,"width":100.,"height":80.},"filter_point_pixel_scale":1,"filter_content_rect_points":{"x":0,"y":0,"width":100,"height":80},"first":{"status":0,"array_available":true,"attachments_available":true,"buffer_available":true,"display_time_available":true,"pts_valid":true,"stream_clock_available":true,"sample_age_at_receipt_ms":6.2,"content_scale":0.9,"scale_factor":1,"content_rect_points":{"x":0,"y":0,"width":90,"height":80}}})
        };
        let write_helper = |path: &Path| {
            fs::write(
                &helper,
                format!(
                    "#!/bin/sh\ncp '{}' \"$3\"\nprintf '%s' '{}'\n",
                    src.display(),
                    sample(path)
                ),
            )
            .unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        };
        write_helper(&image);
        let v = record(&backend, target, &helper, &image, &evidence).unwrap();
        assert_eq!(v["status"], "captured");
        assert_eq!(v["action_allowed"], false);
        assert_eq!(v["image_sha256"], crate::png_sha256(&image).unwrap());
        let bytes = fs::read(&evidence).unwrap();
        let calls = backend.calls.get();
        assert!(record(&backend, target, &helper, &image, &evidence).is_err());
        assert_eq!(backend.calls.get(), calls);
        assert_eq!(fs::read(&evidence).unwrap(), bytes);
        let image2 = dir.join("changed.png");
        let evidence2 = dir.join("changed.json");
        write_helper(&image2);
        let v = record(
            &Fake {
                calls: Cell::new(0),
                change: true,
            },
            target,
            &helper,
            &image2,
            &evidence2,
        )
        .unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["image_ownership"], "unverified");
        assert!(v["error"].as_str().unwrap().contains("changed"));
        assert!(image2.exists());
        let image3 = dir.join("suspended.png");
        let evidence3 = dir.join("suspended.json");
        let mut suspended = sample(&image3);
        suspended["outcome"] = "incomplete".into();
        suspended["first"]["status"] = 3.into();
        suspended["first"]["buffer_available"] = false.into();
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", suspended)).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let v = record(
            &Fake {
                calls: Cell::new(0),
                change: false,
            },
            target,
            &helper,
            &image3,
            &evidence3,
        )
        .unwrap();
        assert_eq!(v["status"], "error");
        assert_eq!(v["sample"]["first"]["status"], 3);
        assert!(!image3.exists());
        let image4 = dir.join("adapted.png");
        write_helper(&image4);
        let stable = Fake {
            calls: Cell::new(0),
            change: false,
        };
        let view = StreamImageView::new(&stable, target, &helper).unwrap();
        let adapted = crate::observe_with_evidence(&view, target, &image4).unwrap();
        assert_eq!(adapted["status"], "captured");
        assert_eq!(adapted["capture"]["capture_type"], "same_sample_stream");
        assert_eq!(adapted["capture"]["stream_frame_status"], 0);
        assert_eq!(adapted["action_allowed"], false);
        assert_eq!(
            adapted["capture"]["geometry_diagnostic"]["content_scale_exactly_one"],
            false
        );
        assert_eq!(
            adapted["capture"]["timing_diagnostic"]["sample_age_at_receipt_ms"],
            6.2
        );
        let outer = dir.join("adapted.png.json");
        let inner = dir.join("adapted.png.stream.json");
        assert!(outer.exists() && inner.exists());
        for p in [
            &helper, &src, &image, &evidence, &image2, &evidence2, &evidence3, &image4, &outer,
            &inner,
        ] {
            fs::remove_file(p).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
}
