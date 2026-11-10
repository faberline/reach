//! Capture and accessibility evidence in one read-only request, never input authority.
use crate::{observe_with_evidence, png_sha256, Backend, Target};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
};

fn sidecar(image: &Path) -> Result<PathBuf, String> {
    let mut name = image.file_name().ok_or("missing filename")?.to_os_string();
    name.push(".binding.json");
    Ok(image.with_file_name(name))
}
fn same_identity(capture: &Value, binding: &Value) -> bool {
    let initial = &capture["process_identity"];
    let before = &binding["evidence"]["process_identity"]["before"];
    let after = &binding["evidence"]["process_identity"]["after"];
    initial["process_start_source"] == "kernel_sysctl_kern_proc"
        && [
            "pid",
            "process_start_unix_seconds",
            "process_start_source",
            "executable",
        ]
        .iter()
        .all(|k| !initial[*k].is_null() && initial[*k] == before[*k] && before[*k] == after[*k])
        && initial["bundle_id"] == before["bundle_id"]
        && before["bundle_id"] == after["bundle_id"]
}
fn consistent(capture: &Value, binding: &Value) -> bool {
    if capture["status"] != "captured"
        || capture["action_allowed"] != false
        || binding["status"] != "unknown"
        || binding["action_allowed"] != false
        || !binding["failed_stage"].is_null()
        || capture["target"]["pid"] != binding["pid"]
        || capture["target"]["window_id"] != binding["window_id"]
        || !same_identity(capture, binding)
    {
        return false;
    }
    let Some(frame) = capture["capture"]["window_frame_points"].as_object() else {
        return false;
    };
    for key in ["cg_before_target", "cg_after_target"] {
        let Some(rows) = binding[key].as_array().filter(|a| a.len() == 1) else {
            return false;
        };
        let row = &rows[0];
        if row["pid"] != binding["pid"]
            || row["window_id"] != binding["window_id"]
            || row["layer"] != 0
            || ["x", "y", "width", "height"].iter().any(|k| {
                frame
                    .get(*k)
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite())
                    .is_none_or(|v| Some(v) != row["bounds"][*k].as_f64())
            })
        {
            return false;
        }
    }
    true
}
/// Reserves a new combined record. A failed or partial operation is recorded when possible.
pub fn observe<B: Backend>(backend: &B, target: Target, image: &Path) -> Result<Value, String> {
    Target::new(target.pid, target.window_id)?;
    if !image.is_absolute() || image.extension().and_then(|x| x.to_str()) != Some("png") {
        return Err("absolute PNG required".into());
    }
    let path = sidecar(image)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|e| format!("cannot reserve combined evidence: {e}"))?;
    let started = Instant::now();
    let mut output = json!({"status":"unknown","action_allowed":false,"consistency":"incomplete","observation_phase":"pending","image_file":image,"combined_evidence_file":path,"capture":null,"binding":null});
    let pending = serde_json::to_vec_pretty(&output).map_err(|e| e.to_string())?;
    file.write_all(&pending)
        .and_then(|_| file.sync_all())
        .map_err(|e| {
            format!(
                "cannot persist pending observation at {}: {e}",
                path.display()
            )
        })?;
    let operation = (|| -> Result<(), String> {
        let capture = observe_with_evidence(backend, target, image)?;
        output["capture"] = capture.clone();
        output["capture_geometry"] = crate::capture_geometry::inspect(&capture["capture"]);
        if capture["status"] != "captured" {
            return Err("capture incomplete; see capture evidence".into());
        }
        let binding = crate::live_ax_cg::observe(backend, target);
        output["binding"] = binding.clone();
        let hash = capture["capture"]["image_sha256"]
            .as_str()
            .ok_or("missing capture hash")?;
        if png_sha256(image)? != hash {
            return Err("image changed after capture".into());
        }
        if !consistent(&capture, &binding) {
            return Err("identity, geometry or binding evidence incomplete or changed".into());
        }
        output["consistency"] = "capture_identity_and_cg_geometry_matched_unverified".into();
        Ok(())
    })();
    if let Err(e) = operation {
        output["error"] = e.into();
    }
    output["observation_phase"] = "finished".into();
    output["elapsed_ms"] = json!(started.elapsed().as_millis());
    output["note"]="Sequential samples are not atomic. Matching does not prove coordinate calibration, occlusion, AX binding, or input safety.".into();
    let bytes = serde_json::to_vec_pretty(&output).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&bytes))
        .and_then(|_| file.set_len(bytes.len() as u64))
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("combined record incomplete at {}: {e}", path.display()))?;
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        let i = json!({"pid":3,"process_start_unix_seconds":123,"process_start_source":"kernel_sysctl_kern_proc","executable":"/a","bundle_id":null});
        let row =
            json!({"pid":3,"window_id":4,"layer":0,"bounds":{"x":0,"y":1,"width":20,"height":30}});
        (
            json!({"status":"captured","action_allowed":false,"target":{"pid":3,"window_id":4},"process_identity":i,"capture":{"window_frame_points":{"x":0,"y":1,"width":20,"height":30}}}),
            json!({"status":"unknown","action_allowed":false,"pid":3,"window_id":4,"evidence":{"process_identity":{"before":i,"after":i}},"cg_before_target":[row],"cg_after_target":[row]}),
        )
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn pending_record_is_saved_before_capture_starts() {
        use std::{fs, path::PathBuf};
        struct FailingBackend {
            record: PathBuf,
        }
        impl Backend for FailingBackend {
            fn identity(&self, _: i32) -> Result<Value, String> {
                let pending: Value =
                    serde_json::from_slice(&fs::read(&self.record).unwrap()).unwrap();
                assert_eq!(pending["observation_phase"], "pending");
                assert_eq!(pending["consistency"], "incomplete");
                assert_eq!(pending["action_allowed"], false);
                Err("intentional identity failure".into())
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<crate::Window>, String> {
                Err("unused".into())
            }
            fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
                Err("unused".into())
            }
        }
        let _guard = crate::TEST_CAPTURE_MUTEX.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("cu-joint-pending-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let image = dir.join("frame.png");
        let record = sidecar(&image).unwrap();
        let result = observe(
            &FailingBackend {
                record: record.clone(),
            },
            Target::new(42, 7).unwrap(),
            &image,
        )
        .unwrap();
        assert_eq!(result["observation_phase"], "finished");
        assert_eq!(result["consistency"], "incomplete");
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&record).unwrap()).unwrap(),
            result
        );
        assert!(!image.exists());
        fs::remove_file(record).unwrap();
        fs::remove_file(dir.join("frame.png.json")).unwrap();
        fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn compares_identity_and_geometry_without_granting_access() {
        let (a, mut b) = fixture();
        assert!(consistent(&a, &b));
        b["cg_after_target"][0]["bounds"]["x"] = 1.into();
        assert!(!consistent(&a, &b));
    }
    #[test]
    fn refuses_missing_or_changed_process_source() {
        let (a, mut b) = fixture();
        b["evidence"]["process_identity"]["after"]["process_start_source"] =
            "appkit_launch_date".into();
        assert!(!consistent(&a, &b));
        let (mut a, mut b) = fixture();
        a["process_identity"]
            .as_object_mut()
            .unwrap()
            .remove("process_start_source");
        for key in ["before", "after"] {
            b["evidence"]["process_identity"][key]
                .as_object_mut()
                .unwrap()
                .remove("process_start_source");
        }
        assert!(!consistent(&a, &b));
    }
    #[test]
    fn refuses_missing_and_failed_evidence() {
        let (a, mut b) = fixture();
        b["failed_stage"] = "accessibility".into();
        assert!(!consistent(&a, &b));
        let (a, mut b) = fixture();
        b["evidence"]["process_identity"]["after"]["executable"] = "/other".into();
        assert!(!consistent(&a, &b));
        let (a, mut b) = fixture();
        b["cg_before_target"] = json!([]);
        assert!(!consistent(&a, &b));
        let (mut a, mut b) = fixture();
        a["capture"]["window_frame_points"]["x"] = Value::Null;
        b["cg_before_target"][0]["bounds"]["x"] = Value::Null;
        assert!(!consistent(&a, &b));
    }
}
