//! `MacBackend`: the macOS implementation of `Backend`, plus the trusted-helper
//! image adapters (`recognize_image`, `recognize_text`).
use super::{capture_lock, windows};
use crate::{bounded_process, display_inventory, image_info, strict_json, Backend, Target, Window};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Run a trusted image adapter. Its unverified result is bound to sampled PNG bytes.
pub fn recognize_image(image: &Path, helper: &Path) -> Result<Value, String> {
    if !image.is_absolute() || !helper.is_absolute() {
        return Err("image and adapter paths must be absolute".into());
    }
    let (before, width, height) = image_info(image)?;
    let output = bounded_process::run(
        Command::new(helper).arg(image),
        std::time::Duration::from_secs(30),
    )?;
    if !output.status.success() {
        return Err("image adapter failed; no result trusted".into());
    }
    let value = strict_json::from_slice(&output.stdout)
        .map_err(|e| format!("invalid adapter JSON: {e}"))?;
    let (after, width_after, height_after) = image_info(image)?;
    if before != after
        || width != width_after
        || height != height_after
        || value["image_sha256"].as_str() != Some(&before)
        || value["image_width"].as_u64() != Some(width as u64)
        || value["image_height"].as_u64() != Some(height as u64)
        || value["status"] != "review_required"
        || value["action_allowed"] != false
    {
        return Err("adapter result or image identity is invalid or changed".into());
    }
    Ok(value)
}

/// Read an existing PNG with a trusted local OCR helper. Never authorizes input.
pub fn recognize_text(image: &Path, helper: &Path) -> Result<Value, String> {
    let value = recognize_image(image, helper)?;
    validate_ocr_items(&value)?;
    Ok(value)
}

// Validate the helper's image-space format, not recognition accuracy or click coordinates.
pub(crate) fn validate_ocr_items(value: &Value) -> Result<(), String> {
    let invalid = || "OCR result has invalid items".to_string();
    let items = value["items"]
        .as_array()
        .filter(|v| v.len() <= 1000)
        .ok_or_else(invalid)?;
    for item in items {
        if item["text"].as_str().is_none() {
            return Err(invalid());
        }
        let confidence = item["confidence_uncalibrated"]
            .as_f64()
            .ok_or_else(invalid)?;
        if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
            return Err(invalid());
        }
        let rect = &item["box_normalized_bottom_left"];
        let coordinate = |name: &str| {
            rect[name]
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(invalid)
        };
        let (x, y, w, h) = (
            coordinate("x")?,
            coordinate("y")?,
            coordinate("width")?,
            coordinate("height")?,
        );
        // Allow small floating-point rounding at image boundaries.
        let eps = 0.000001;
        if x < -eps || y < -eps || w <= 0.0 || h <= 0.0 || x + w > 1.0 + eps || y + h > 1.0 + eps {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(crate) fn validate_all_capture_scope(metadata: &Value) -> Result<(), String> {
    if metadata["window_scope"] != "all_window_candidates" {
        return Err("capture helper did not confirm all-window scope".into());
    }
    if metadata["onscreen_before"].as_bool().is_none()
        || metadata["onscreen_before"] != metadata["onscreen_after"]
    {
        return Err("capture helper did not confirm consistent onscreen samples".into());
    }
    Ok(())
}

pub struct MacBackend {
    capture_helper: PathBuf,
    ax_helper: PathBuf,
}

impl MacBackend {
    pub fn new(capture_helper: PathBuf, ax_helper: PathBuf) -> Result<Self, String> {
        if !capture_helper.is_absolute() || !ax_helper.is_absolute() {
            return Err("backend helper paths must be absolute".into());
        }
        Ok(Self {
            capture_helper,
            ax_helper,
        })
    }

    pub fn adjacent_to_executable() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = executable.parent().ok_or("no executable directory")?;
        Self::new(dir.join("mac-window-capture"), dir.join("mac-ax-snapshot"))
    }
}

impl Backend for MacBackend {
    fn accessibility_subtree(
        &self,
        pid: i32,
        source: &str,
        path: &[usize],
    ) -> Result<Value, String> {
        if pid <= 0
            || !matches!(source, "main" | "focused" | "windows")
            || path.len() > 8
            || path.iter().any(|i| *i >= 160)
            || (source == "windows" && path.first().is_none_or(|i| *i >= 32))
        {
            return Err("invalid accessibility subtree target".into());
        }
        let indexes = if path.is_empty() {
            "-".to_owned()
        } else {
            path.iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        let output = bounded_process::run(
            Command::new(&self.ax_helper)
                .arg("--subtree")
                .arg(pid.to_string())
                .arg(source)
                .arg(indexes),
            std::time::Duration::from_secs(15),
        )
        .map_err(|e| format!("AX subtree helper failed: {e}"))?;
        if !output.status.success() {
            return Err("AX subtree helper could not read branch".into());
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid AX subtree response: {e}"))?;
        if value["status"] != "ok"
            || value["action_allowed"] != false
            || value["process_start_source"] != "kernel_sysctl_kern_proc"
            || value["pid"].as_i64() != Some(i64::from(pid))
            || value["source"] != source
            || value["path"] != json!(path)
            || value["scope"] != "read_only_subtree_unverified"
            || value["cg_window_binding"] != "unverified"
            || value["node"].as_object().is_none()
            || value["truncated"].as_bool().is_none()
        {
            return Err("unsafe or mismatched AX subtree response".into());
        }
        Ok(value)
    }
    fn display_inventory(&self) -> Result<Value, String> {
        display_inventory::observe().map_err(str::to_owned)
    }
    fn identity(&self, pid: i32) -> Result<Value, String> {
        if pid <= 0 {
            return Err("PID must be positive".into());
        }
        let output = bounded_process::run(
            Command::new(&self.ax_helper)
                .arg("--identity")
                .arg(pid.to_string()),
            std::time::Duration::from_secs(15),
        )
        .map_err(|e| format!("identity helper could not start: {e}"))?;
        if !output.status.success() {
            return Err("process identity unavailable".into());
        }
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid process identity response: {e}"))?;
        if value["process_start_source"] != "kernel_sysctl_kern_proc" {
            return Err("identity helper has missing or unsupported process start source".into());
        }
        Ok(value)
    }
    fn windows(&self, pid: Option<i32>) -> Result<Vec<Window>, String> {
        if pid.is_some_and(|p| p <= 0) {
            return Err("PID must be positive".into());
        }
        windows::onscreen_windows(pid)
    }

    fn all_windows(&self, pid: i32) -> Result<(Vec<Window>, usize), String> {
        windows::all_windows(pid)
    }

    fn exact_window(&self, target: Target) -> Result<Window, String> {
        windows::exact_window(target)
    }

    fn capture(&self, target: Target, output: &Path) -> Result<Value, String> {
        let _capture_lock = capture_lock::acquire()?;
        let result = bounded_process::run(
            Command::new(&self.capture_helper)
                .arg(target.window_id.to_string())
                .arg(target.pid.to_string())
                .arg(output),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| format!("capture helper could not start: {e}"))?;
        if !result.status.success() {
            return Err(format!(
                "capture failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let mut metadata: Value = serde_json::from_slice(&result.stdout)
            .map_err(|e| format!("invalid capture metadata: {e}"))?;
        metadata["image_sha256"] = image_info(output)?.0.into();
        Ok(metadata)
    }

    fn capture_all(&self, target: Target, output: &Path) -> Result<Value, String> {
        let _capture_lock = capture_lock::acquire()?;
        let result = bounded_process::run(
            Command::new(&self.capture_helper)
                .arg("--include-offscreen")
                .arg(target.window_id.to_string())
                .arg(target.pid.to_string())
                .arg(output),
            std::time::Duration::from_secs(30),
        )
        .map_err(|e| format!("capture helper could not start: {e}"))?;
        if !result.status.success() {
            return Err(format!(
                "capture failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        let mut metadata: Value = serde_json::from_slice(&result.stdout)
            .map_err(|e| format!("invalid capture metadata: {e}"))?;
        validate_all_capture_scope(&metadata)?;
        metadata["image_sha256"] = image_info(output)?.0.into();
        Ok(metadata)
    }

    fn accessibility(&self, pid: i32) -> Result<Value, String> {
        if pid <= 0 {
            return Err("PID must be positive".into());
        }
        let output = bounded_process::run(
            Command::new(&self.ax_helper).arg(pid.to_string()),
            std::time::Duration::from_secs(15),
        )
        .map_err(|e| format!("AX helper could not start: {e}"))?;
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid AX response: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "AX snapshot failed: {}",
                value["reason"].as_str().unwrap_or("unknown error")
            ));
        }
        if value["status"] != "ok"
            || value["pid"].as_i64() != Some(pid.into())
            || value["process_start_source"] != "kernel_sysctl_kern_proc"
            || value["cg_window_binding"] != "unverified"
            || value["action_allowed"] != false
        {
            return Err("AX snapshot identity or safety fields do not match".into());
        }
        Ok(value)
    }

    fn accessibility_windows(&self, pid: i32) -> Result<Value, String> {
        if pid <= 0 {
            return Err("PID must be positive".into());
        }
        let output = bounded_process::run(
            Command::new(&self.ax_helper)
                .arg("--windows-only")
                .arg(pid.to_string()),
            std::time::Duration::from_secs(15),
        )
        .map_err(|e| format!("AX helper could not start: {e}"))?;
        let value: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("invalid AX response: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "AX snapshot failed: {}",
                value["reason"].as_str().unwrap_or("unknown error")
            ));
        }
        if value["tree_scope"] != "windows_only"
            || value["status"] != "ok"
            || value["pid"].as_i64() != Some(pid.into())
            || value["process_start_source"] != "kernel_sysctl_kern_proc"
            || value["cg_window_binding"] != "unverified"
            || value["action_allowed"] != false
        {
            return Err("AX snapshot identity or safety fields do not match".into());
        }
        Ok(value)
    }
}
