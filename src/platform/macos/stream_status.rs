//! Read-only stream metadata. A sample never grants input authority.
use crate::{Backend, Target, Window};
use serde_json::{json, Value};
use std::{
    fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path, process::Command,
    time::Duration,
};

pub(crate) fn window_json(w: &Window) -> Value {
    json!({"pid":w.target.pid,"window_id":w.target.window_id,"layer":w.layer,
        "bounds":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height}})
}
pub(crate) fn valid_identity(v: &Value, pid: i32) -> bool {
    v["status"] == "ok"
        && v["action_allowed"] == false
        && v["pid"].as_i64() == Some(pid.into())
        && v["process_start_source"] == "kernel_sysctl_kern_proc"
        && v["process_start_unix_seconds"]
            .as_f64()
            .is_some_and(|x| x.is_finite() && x > 0.0)
        && v["executable"]
            .as_str()
            .is_some_and(|x| x.starts_with('/') && !x.chars().any(char::is_control))
}
fn validate_sample(v: &Value, target: Target) -> Result<bool, String> {
    validate_sample_scope(v, target, "stream_metadata_only")
}
pub(crate) fn validate_sample_scope(
    v: &Value,
    target: Target,
    scope: &str,
) -> Result<bool, String> {
    if v["schema"] != "cu-stream-status-v1"
        || v["scope"] != scope
        || v["pid"].as_i64() != Some(target.pid.into())
        || v["window_id"].as_u64() != Some(target.window_id.into())
        || v["input_posted"] != false
        || v["action_allowed"] != false
        || v["onscreen_before"].as_bool().is_none()
        || v["onscreen_before"] != v["onscreen_after"]
        || v["received"].as_bool().is_none()
    {
        return Err("invalid or mismatched stream metadata".into());
    }
    if v["received"] != true {
        return Err("stream sample unavailable".into());
    }
    let f = &v["first"];
    let status = f["status"]
        .as_i64()
        .filter(|n| (0..=5).contains(n))
        .ok_or("invalid stream frame status")?;
    for key in [
        "array_available",
        "attachments_available",
        "buffer_available",
        "display_time_available",
        "pts_valid",
    ] {
        if f[key].as_bool().is_none() {
            return Err(format!("invalid stream field {key}"));
        }
    }
    if f["array_available"] != true
        || f["attachments_available"] != true
        || f["display_time_available"] != true
    {
        return Err("stream frame metadata incomplete".into());
    }
    Ok(status == 0 && f["buffer_available"] == true && f["pts_valid"] == true)
}

/// Reserve evidence first. Trusted helpers can execute arbitrary code; caller must trust the path.
pub fn record<B: Backend>(
    backend: &B,
    target: Target,
    helper: &Path,
    output: &Path,
) -> Result<Value, String> {
    Target::new(target.pid, target.window_id)?;
    if !helper.is_absolute() || !output.is_absolute() {
        return Err("absolute paths required".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)
        .map_err(|e| format!("cannot reserve evidence: {e}"))?;
    let mut record = json!({"schema":"cu-stream-evidence-v1","target":{"pid":target.pid,"window_id":target.window_id},
        "action_allowed":false,"sample":null,"identity_before":null,"identity_after":null,"window_before":null,"window_after":null,
        "sample_complete":false,"error":null,"note":"Metadata is sequential and not bound to a screenshot; frame content, freshness and input isolation are unverified."});
    let observed = (|| -> Result<bool, String> {
        let _lock = crate::capture_lock::acquire()?;
        let before = backend.identity(target.pid)?;
        record["identity_before"] = before.clone();
        if !valid_identity(&before, target.pid) {
            return Err("initial process identity invalid".into());
        }
        let w1 = backend.exact_window(target)?;
        record["window_before"] = window_json(&w1);
        if w1.target != target || w1.layer != Some(0) || !w1.bounds.valid() {
            return Err("initial window invalid".into());
        }
        let output = crate::bounded_process::run(
            Command::new(helper)
                .arg(target.pid.to_string())
                .arg(target.window_id.to_string()),
            Duration::from_secs(20),
        )?;
        if !output.status.success() {
            return Err(format!(
                "stream helper failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let sample: Value = crate::strict_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid stream JSON: {e}"))?;
        record["sample"] = sample.clone();
        let complete = validate_sample(&sample, target)?;
        let w2 = backend.exact_window(target)?;
        record["window_after"] = window_json(&w2);
        let after = backend.identity(target.pid)?;
        record["identity_after"] = after.clone();
        if !valid_identity(&after, target.pid)
            || before != after
            || w1.target != w2.target
            || w1.layer != w2.layer
            || w1.bounds != w2.bounds
        {
            return Err("process or window changed during sample".into());
        }
        Ok(complete)
    })();
    match observed {
        Ok(complete) => record["sample_complete"] = complete.into(),
        Err(error) => record["error"] = error.into(),
    }
    let bytes = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("evidence may be partial at {}: {e}", output.display()))?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_changed_and_noncomplete_samples() {
        let t = Target::new(42, 99).unwrap();
        let mut v = json!({"schema":"cu-stream-status-v1","scope":"stream_metadata_only","pid":42,"window_id":99,"input_posted":false,"action_allowed":false,
       "onscreen_before":true,"onscreen_after":true,"received":true,"first":{"status":0,"array_available":true,"attachments_available":true,"buffer_available":true,"display_time_available":true,"pts_valid":true}});
        assert_eq!(validate_sample(&v, t), Ok(true));
        v["first"]["status"] = 3.into();
        v["first"]["buffer_available"] = false.into();
        assert_eq!(validate_sample(&v, t), Ok(false));
        v["onscreen_after"] = false.into();
        assert!(validate_sample(&v, t).is_err());
        v["onscreen_after"] = true.into();
        v["first"]["attachments_available"] = false.into();
        assert!(validate_sample(&v, t).is_err());
        v["first"]["attachments_available"] = true.into();
        v["first"]["status"] = 99.into();
        assert!(validate_sample(&v, t).is_err());
        v["first"]["status"] = 0.into();
        v["input_posted"] = true.into();
        assert!(validate_sample(&v, t).is_err());
    }
    #[test]
    fn records_success_and_identity_change_without_overwrite() {
        let _guard = crate::TEST_CAPTURE_MUTEX.lock().unwrap();
        use std::{cell::Cell, fs, os::unix::fs::PermissionsExt};
        struct Fake {
            calls: Cell<u32>,
            change: bool,
        }
        impl Backend for Fake {
            fn identity(&self, pid: i32) -> Result<Value, String> {
                let n = self.calls.get();
                self.calls.set(n + 1);
                Ok(
                    json!({"status":"ok","action_allowed":false,"pid":pid,"process_start_source":"kernel_sysctl_kern_proc",
           "process_start_unix_seconds":if self.change && n>0 {124.0} else {123.0},"executable":"/test"}),
                )
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
                Err("unused".into())
            }
            fn exact_window(&self, target: Target) -> Result<Window, String> {
                Ok(Window {
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
        let dir = std::env::temp_dir().join(format!("cu-stream-record-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let helper = dir.join("helper");
        let first = dir.join("first.json");
        let changed = dir.join("changed.json");
        let response = json!({"schema":"cu-stream-status-v1","scope":"stream_metadata_only","pid":42,"window_id":99,"input_posted":false,"action_allowed":false,
       "onscreen_before":true,"onscreen_after":true,"received":true,"first":{"status":0,"array_available":true,"attachments_available":true,"buffer_available":true,"display_time_available":true,"pts_valid":true}});
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", response)).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let backend = Fake {
            calls: Cell::new(0),
            change: false,
        };
        let target = Target::new(42, 99).unwrap();
        let r = record(&backend, target, &helper, &first).unwrap();
        assert_eq!(r["sample_complete"], true);
        assert_eq!(r["action_allowed"], false);
        let original = fs::read(&first).unwrap();
        let count = backend.calls.get();
        assert!(record(&backend, target, &helper, &first).is_err());
        assert_eq!(backend.calls.get(), count);
        assert_eq!(fs::read(&first).unwrap(), original);
        let backend = Fake {
            calls: Cell::new(0),
            change: true,
        };
        let r = record(&backend, target, &helper, &changed).unwrap();
        assert_eq!(r["sample_complete"], false);
        assert!(r["error"].as_str().unwrap().contains("changed"));
        assert_eq!(r["sample"]["first"]["status"], 0);
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&changed).unwrap()).unwrap(),
            r
        );
        fs::remove_file(first).unwrap();
        fs::remove_file(changed).unwrap();
        fs::remove_file(helper).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
