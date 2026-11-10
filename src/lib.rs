//! App-agnostic macOS observation primitives. No global input is posted here.
pub mod bounded_process;
/// Time budgets for one read-only observer run and its two sequential model calls.
/// Filesystem work and detached descendants can still exceed these budgets.
pub mod observer_timeouts {
    use std::time::Duration;
    pub const MODEL: Duration = Duration::from_secs(30);
    pub const OBSERVER: Duration = Duration::from_secs(75);
    #[cfg(test)]
    mod tests {
        #[test]
        fn outer_budget_covers_two_model_deadlines_and_margin() {
            assert!(super::OBSERVER >= super::MODEL * 2 + std::time::Duration::from_secs(10));
        }
    }
}
pub mod simulate;
pub mod strict_json;
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(unix)]
pub mod agent_client;
pub mod agent_protocol;
pub mod ax_cg;
pub mod calibration;
pub mod capture_geometry;
pub mod cursor_overlay;
pub mod display_inventory;
pub mod input_guard;
pub mod interpretation;
pub mod joint_observation;
pub mod live_ax_cg;
pub mod native_event_preview;
pub mod overlay_session;
pub mod platform;
#[cfg(all(test, target_os = "macos"))]
pub(crate) static TEST_CAPTURE_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
pub mod virtual_input;

// Public and crate-internal paths of the macOS adapter stay where they were.
#[cfg(target_os = "macos")]
pub use platform::macos::backend::{recognize_image, recognize_text, MacBackend};
#[cfg(all(test, target_os = "macos"))]
pub(crate) use platform::macos::backend::{validate_all_capture_scope, validate_ocr_items};
#[cfg(target_os = "macos")]
pub(crate) use platform::macos::capture_lock;
#[cfg(target_os = "macos")]
use platform::macos::codec::{png_dimensions, sha256_bytes};
#[cfg(target_os = "macos")]
pub use platform::macos::{stream_image, stream_status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub pid: i32,
    pub window_id: u32,
}

impl Target {
    pub fn new(pid: i32, window_id: u32) -> Result<Self, String> {
        if pid <= 0 || window_id == 0 {
            return Err("PID and window ID must be positive".into());
        }
        Ok(Self { pid, window_id })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
            && (self.x + self.width).is_finite()
            && (self.y + self.height).is_finite()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub target: Target,
    pub bounds: Rect,
    /// Core Graphics layer. Missing or unparseable values remain unknown.
    pub layer: Option<i32>,
    /// Optional OS-provided name. It is not a unique window identity.
    pub title: Option<String>,
}
impl Window {
    pub fn surface_class(&self) -> &'static str {
        match self.layer {
            Some(0) => "normal_layer_candidate",
            Some(_) => "non_normal_layer",
            None => "unknown_layer",
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn png_dimensions(_data: &[u8]) -> Option<(usize, usize)> {
    None
}

#[cfg(not(target_os = "macos"))]
fn sha256_bytes(_data: &[u8]) -> Result<String, String> {
    Err("native SHA-256 is unavailable on this platform".into())
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}
#[cfg(not(unix))]
fn same_file(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    false
}

const MAX_IMAGE_BYTES: u64 = 256_000_000;

/// Read a bounded regular file and reject observed final-component symlinks.
/// Identity and metadata checks cannot rule out every race.
pub fn read_regular_file_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    if limit == 0 || limit > MAX_IMAGE_BYTES {
        return Err("invalid file size limit".into());
    }
    let path_metadata =
        fs::symlink_metadata(path).map_err(|e| format!("cannot inspect captured image: {e}"))?;
    if !path_metadata.file_type().is_file() || path_metadata.len() > limit {
        return Err("captured image is not a supported regular file or is too large".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("cannot open captured image: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("cannot inspect open image: {e}"))?;
    if !metadata.is_file() || metadata.len() > limit || !same_file(&path_metadata, &metadata) {
        return Err("captured image is not a supported regular file or is too large".into());
    }
    let mut data = Vec::new();
    (&file)
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("cannot read captured image: {e}"))?;
    let after = file
        .metadata()
        .map_err(|e| format!("cannot recheck captured image: {e}"))?;
    let path_after =
        fs::symlink_metadata(path).map_err(|e| format!("cannot recheck image path: {e}"))?;
    if !path_after.file_type().is_file()
        || !same_file(&metadata, &path_after)
        || data.len() as u64 > limit
        || data.len() as u64 != metadata.len()
        || metadata.len() != after.len()
        || metadata.modified().ok() != after.modified().ok()
    {
        return Err("captured image changed during read or is too large".into());
    }
    Ok(data)
}

fn read_image_bounded(path: &Path) -> Result<Vec<u8>, String> {
    read_regular_file_bounded(path, MAX_IMAGE_BYTES)
}

fn image_info(path: &Path) -> Result<(String, usize, usize), String> {
    let data = read_image_bounded(path)?;
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("captured image is not a supported PNG".into());
    }
    let (width, height) = png_dimensions(&data).ok_or("captured image is not a supported PNG")?;
    let sha = sha256_bytes(&data)?;
    Ok((sha, width, height))
}

/// Validate and hash a PNG through the same bounded path used by capture.
/// The digest binds bytes observed during this call, not later file contents.
pub fn png_sha256(path: &Path) -> Result<String, String> {
    image_info(path).map(|(sha, _, _)| sha)
}

/// Hash a regular file with the same size and identity checks as captured images.
/// A matching digest only describes sampled bytes; it does not establish trust.
pub fn regular_file_sha256(path: &Path) -> Result<String, String> {
    sha256_bytes(&read_image_bounded(path)?)
}

/// Hash a caller-owned byte slice. No provenance or trust is implied.
pub fn bytes_sha256(data: &[u8]) -> Result<String, String> {
    sha256_bytes(data)
}

fn evidence_path(image: &Path) -> Result<PathBuf, String> {
    let filename = image
        .file_name()
        .ok_or("image has no filename")?
        .to_string_lossy();
    Ok(image.with_file_name(format!("{filename}.json")))
}

/// Create a new sidecar record without overwriting existing evidence.
pub fn save_evidence(image: &Path, report: &Value) -> Result<PathBuf, String> {
    let path = evidence_path(image)?;
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|e| format!("cannot create evidence for {}: {e}", image.display()))?;
    let written = file.write_all(&bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = written {
        return Err(format!(
            "evidence write failed at {}; partial evidence may remain; no image was deleted: {e}",
            path.display()
        ));
    }
    Ok(path)
}

/// Backends supply observations; input is unsupported unless separately verified.
/// Capture backends must create the output exclusively and return its SHA-256.
pub trait Backend {
    fn identity(&self, pid: i32) -> Result<Value, String>;
    /// Labels evidence scope only; this cannot grant access or verify the backend.
    fn inventory_scope(&self) -> &'static str {
        "onscreen_candidates"
    }
    fn windows(&self, pid: Option<i32>) -> Result<Vec<Window>, String>;
    /// Diagnostic candidates, including windows outside the onscreen inventory.
    fn all_windows(&self, _pid: i32) -> Result<(Vec<Window>, usize), String> {
        Err("all-window inventory is not supported by this backend".into())
    }
    /// Query a specified target. The default rejects an incomplete inventory.
    fn exact_window(&self, target: Target) -> Result<Window, String> {
        let (windows, omitted) = self.all_windows(target.pid)?;
        if omitted != 0 {
            return Err(format!(
                "all-window inventory incomplete: {omitted} omitted"
            ));
        }
        let mut found = windows.into_iter().filter(|w| w.target == target);
        let first = found.next().ok_or("target window missing")?;
        if found.next().is_some() {
            return Err("target window ambiguous".into());
        }
        Ok(first)
    }
    fn capture(&self, target: Target, output: &Path) -> Result<Value, String>;
    /// Explicit read-only capture from the all-window inventory, including offscreen candidates.
    fn capture_all(&self, _target: Target, _output: &Path) -> Result<Value, String> {
        Err("all-window capture is not supported by this backend".into())
    }
    /// Read-only display evidence. It does not calibrate coordinates or authorize input.
    fn display_inventory(&self) -> Result<Value, String> {
        Err("display inventory is not supported by this backend".into())
    }
    fn accessibility(&self, _pid: i32) -> Result<Value, String> {
        Err("accessibility is not supported by this backend".into())
    }
    /// A fresh read-only branch; path indexes are not stable element identities.
    fn accessibility_subtree(
        &self,
        _pid: i32,
        _source: &str,
        _path: &[usize],
    ) -> Result<Value, String> {
        Err("accessibility subtree is not supported by this backend".into())
    }
    /// A backend may provide only top-level window metadata without children.
    fn accessibility_windows(&self, pid: i32) -> Result<Value, String> {
        self.accessibility(pid)
    }
}

pub fn inspect_target<B: Backend>(backend: &B, target: Target) -> Result<Window, String> {
    let mut found = backend
        .windows(Some(target.pid))?
        .into_iter()
        .filter(|w| w.target == target && w.bounds.valid());
    let first = found
        .next()
        .ok_or("target window is not currently visible")?;
    if found.next().is_some() {
        return Err("window identity is ambiguous".into());
    }
    if first.layer != Some(0) {
        return Err("window layer is unknown or not normal; target not accepted".into());
    }
    Ok(first)
}

/// Version of the observation record. 2 adds `window_before` and `window_after`.
const EVIDENCE_VERSION: u32 = 2;

fn window_snapshot(w: &Window) -> Value {
    json!({"pid":w.target.pid, "window_id":w.target.window_id, "layer":w.layer, "title":w.title,
        "bounds_points":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height}})
}

pub fn observe<B: Backend>(backend: &B, target: Target, output: &Path) -> Result<Value, String> {
    if !output.is_absolute() || output.extension().and_then(|s| s.to_str()) != Some("png") {
        return Err("output must be an absolute path ending in .png".into());
    }
    match fs::symlink_metadata(output) {
        Ok(_) => return Err("output already exists".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(format!("cannot inspect output path: {e}")),
    }
    let identity = backend.identity(target.pid)?;
    let started = identity["process_start_unix_seconds"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0);
    if identity["status"] != "ok"
        || identity["pid"].as_i64() != Some(target.pid.into())
        || started.is_none()
    {
        return Err("process identity is missing or invalid".into());
    }
    let before = inspect_target(backend, target)?;
    let metadata = backend.capture(target, output)?;
    let after = inspect_target(backend, target)?;
    if before.target != after.target || before.bounds != after.bounds || before.layer != after.layer
    {
        return Err(
            "window identity, layer or geometry changed during capture; image must not drive input"
                .into(),
        );
    }
    if backend.identity(target.pid)? != identity {
        return Err("process identity changed during capture; image must not drive input".into());
    }
    if metadata["window_id"].as_u64() != Some(target.window_id.into())
        || metadata["owner_pid"].as_i64() != Some(target.pid.into())
        || metadata["image_path"].as_str() != output.to_str()
    {
        return Err("capture metadata does not match target or output".into());
    }
    let frame = &metadata["window_frame_points"];
    if frame["x"].as_f64() != Some(before.bounds.x)
        || frame["y"].as_f64() != Some(before.bounds.y)
        || frame["width"].as_f64() != Some(before.bounds.width)
        || frame["height"].as_f64() != Some(before.bounds.height)
    {
        return Err(
            "capture frame does not match window inventory; image must not drive input".into(),
        );
    }
    let expected_sha = metadata["image_sha256"]
        .as_str()
        .ok_or("capture metadata has no image SHA-256")?;
    if expected_sha.len() != 64
        || !expected_sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("capture metadata has invalid image SHA-256".into());
    }
    let (actual_sha, width, height) = image_info(output)?;
    if actual_sha != expected_sha {
        return Err("captured image SHA-256 does not match metadata".into());
    }
    if metadata["image_width"].as_u64() != Some(width as u64)
        || metadata["image_height"].as_u64() != Some(height as u64)
    {
        return Err("captured image dimensions do not match metadata".into());
    }
    Ok(
        json!({"status":"captured", "decision":"unknown", "action_allowed":false,
        "reason":"no app-specific recognizer is configured", "target":{"pid":target.pid,"window_id":target.window_id},
        "process_identity":identity, "window_layer":before.layer, "surface_class":before.surface_class(),
        "evidence_version":EVIDENCE_VERSION,
        "window_before":window_snapshot(&before), "window_after":window_snapshot(&after),
        "capture":metadata, "note":"normal layer is only a candidate; visibility, focus and surface type are not proven"}),
    )
}

/// Reserve a new sidecar before capture and record either the result or its failure.
/// A failed capture never claims ownership of a file that may appear at `output`.
/// The caller must treat `status=error` as a failed observation, even if this
/// function successfully writes a record and returns `Ok`.
pub fn observe_with_evidence<B: Backend>(
    backend: &B,
    target: Target,
    output: &Path,
) -> Result<Value, String> {
    observe_with_evidence_scoped(backend, target, output, backend.inventory_scope())
}

fn observe_with_evidence_scoped<B: Backend>(
    backend: &B,
    target: Target,
    output: &Path,
    scope: &str,
) -> Result<Value, String> {
    Target::new(target.pid, target.window_id)?;
    if !output.is_absolute() || output.extension().and_then(|s| s.to_str()) != Some("png") {
        return Err("output must be an absolute path ending in .png".into());
    }
    match fs::symlink_metadata(output) {
        Ok(_) => return Err("output already exists".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(format!("cannot inspect output path: {e}")),
    }
    let path = evidence_path(output)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut sidecar = options
        .open(&path)
        .map_err(|e| format!("cannot reserve evidence at {}: {e}", path.display()))?;
    let observation = observe(backend, target, output);
    let mut report = match observation {
        Ok(value) => value,
        Err(reason) => json!({"status":"error", "decision":"unknown", "action_allowed":false,
            "evidence_version":EVIDENCE_VERSION,
            "target":{"pid":target.pid, "window_id":target.window_id},
            "requested_image_path":output, "image_ownership":"unverified", "error":reason}),
    };
    report["inventory_scope"] = scope.into();
    report["evidence_file"] = path.to_string_lossy().into_owned().into();
    let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    sidecar.write_all(&bytes).and_then(|_| sidecar.sync_all()).map_err(|e| {
        format!("cannot save evidence at {}; partial evidence may remain; no image was deleted: {e}", path.display())
    })?;
    Ok(report)
}

/// A read-only view bound to one target, including offscreen candidates.
pub struct AllWindowView<'a, T: Backend> {
    backend: &'a T,
    target: Target,
}
impl<'a, T: Backend> AllWindowView<'a, T> {
    pub fn new(backend: &'a T, target: Target) -> Result<Self, String> {
        Target::new(target.pid, target.window_id)?;
        Ok(Self { backend, target })
    }
}
impl<T: Backend> Backend for AllWindowView<'_, T> {
    fn inventory_scope(&self) -> &'static str {
        "exact_window_candidate"
    }
    fn identity(&self, pid: i32) -> Result<Value, String> {
        if pid != self.target.pid {
            return Err("target PID mismatch".into());
        }
        self.backend.identity(pid)
    }
    fn windows(&self, pid: Option<i32>) -> Result<Vec<Window>, String> {
        if pid != Some(self.target.pid) {
            return Err("target PID mismatch".into());
        }
        Ok(vec![self.backend.exact_window(self.target)?])
    }
    fn capture(&self, target: Target, output: &Path) -> Result<Value, String> {
        if target != self.target {
            return Err("capture target mismatch".into());
        }
        self.backend.capture_all(target, output)
    }
}

/// Uses the same safety checks as onscreen observation but a separate inventory
/// and capture path. No result verifies visibility, freshness, or input safety.
pub fn observe_all_with_evidence<B: Backend>(
    backend: &B,
    target: Target,
    output: &Path,
) -> Result<Value, String> {
    observe_with_evidence_scoped(
        &AllWindowView::new(backend, target)?,
        target,
        output,
        "exact_window_candidate",
    )
}

pub fn unsupported_action() -> Value {
    json!({"status":"unsupported", "action_allowed":false,
        "reason":"background input that preserves the user's pointer and focus has not been verified"})
}

/// Static implementation facts, not a live check of permissions or targets.
/// No OS APIs, helper processes, captures, or input are used to create this report.
pub fn capabilities() -> Value {
    json!({
        "schema_version": 1,
        "status": "ok",
        "report_kind": "static_implementation",
        "runtime_permissions_checked": false,
        "target_checked": false,
        "action_allowed": false,
        "platform": {"compiled_for_macos": cfg!(target_os = "macos")},
        "observation": {
            "window_inventory": "implemented_on_macos",
            "window_capture": "implemented_on_macos_requires_helper_and_permission",
            "accessibility_snapshot": "implemented_on_macos_requires_helper_and_permission",
            "ax_cg_read_only_observation": "implemented_but_binding_and_calibration_unverified",
            "accessibility_to_cg_window_binding": "unverified",
            "target_gui_session_binding": "unverified",
            "screen_to_window_coordinate_mapping": "unverified",
            "app_specific_recognition": "not_provided"
        },
        "input": {
            "desktop": "unsupported",
            "foreground": "unsupported",
            "background_window": "unsupported",
            "keyboard": "unsupported",
            "accessibility_action": "unsupported",
            "pointer_preservation": "unverified",
            "focus_preservation": "unverified",
            "offline_input_guard": "preflight_only_does_not_authorize_input"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(target_os = "macos")]
    #[test]
    fn all_capture_requires_typed_consistent_visibility_and_scope() {
        let mut record = json!({"window_scope":"all_window_candidates","onscreen_before":false,"onscreen_after":false});
        assert!(validate_all_capture_scope(&record).is_ok());
        record["onscreen_before"] = true.into();
        assert!(validate_all_capture_scope(&record).is_err());
        record["onscreen_after"] = true.into();
        assert!(validate_all_capture_scope(&record).is_ok());
        record["onscreen_before"] = Value::Null;
        assert!(validate_all_capture_scope(&record).is_err());
        record["onscreen_before"] = true.into();
        record["window_scope"] = "onscreen_candidates".into();
        assert!(validate_all_capture_scope(&record).is_err());
    }

    // A valid 1×1 RGBA PNG, including CRCs and compressed pixel data.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60,
        0x60, 0x60, 0xf8, 0x0f, 0x00, 0x01, 0x04, 0x01, 0x00, 0x5f, 0xe5, 0xc3, 0x4b, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    // SHA-256 of `PNG`. The fakes report it directly so that the checks that run
    // before image decoding also run on platforms without the native decoder.
    const PNG_SHA256: &str = "abc58d5127d7cdf313beb9ec8ee839860a9c6bfbc48c8b8eb6a3f7d8bb63de6f";

    #[cfg(target_os = "macos")]
    #[test]
    fn png_sha256_constant_matches_the_native_decoder() {
        let image = TestImage::new();
        fs::write(image.path(), PNG).unwrap();
        assert_eq!(image_info(image.path()).unwrap().0, PNG_SHA256);
    }

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    struct TestImage(PathBuf);
    impl TestImage {
        fn new() -> Self {
            loop {
                let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
                let dir = std::env::temp_dir()
                    .join(format!("mac-cu-observe-{}-{id}", std::process::id()));
                match fs::create_dir(&dir) {
                    Ok(()) => return Self(dir.join("frame.png")),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("cannot create test directory: {e}"),
                }
            }
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestImage {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ocr_requires_matching_image_digest_and_safe_status() {
        use std::os::unix::fs::PermissionsExt;
        let img = TestImage::new();
        fs::write(img.path(), PNG).unwrap();
        let hash = image_info(img.path()).unwrap().0;
        let helper = img.path().with_file_name("helper");
        let write_helper = |digest: &str, allowed: bool| {
            let value = json!({"status":"review_required", "action_allowed":allowed,
                "image_sha256":digest, "image_width":1, "image_height":1, "items":[]});
            fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", value)).unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        };
        write_helper(&hash, false);
        assert_eq!(
            recognize_text(img.path(), &helper).unwrap()["action_allowed"],
            false
        );
        assert_eq!(
            recognize_image(img.path(), &helper).unwrap()["image_sha256"],
            hash
        );
        write_helper(&"0".repeat(64), false);
        assert!(recognize_text(img.path(), &helper).is_err());
        assert!(recognize_image(img.path(), &helper).is_err());
        write_helper(&hash, true);
        assert!(recognize_text(img.path(), &helper).is_err());
        assert!(recognize_text(img.path(), Path::new("relative")).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn ocr_rejects_malformed_text_confidence_and_boxes() {
        let valid = json!({"text":"測試","confidence_uncalibrated":0.5,
            "box_normalized_bottom_left":{"x":0.1,"y":0.2,"width":0.3,"height":0.4}});
        assert!(validate_ocr_items(&json!({"items":[valid.clone()]})).is_ok());
        assert!(validate_ocr_items(&json!({"items":[]})).is_ok());
        for (field, bad) in [
            ("text", json!(42)),
            ("confidence_uncalibrated", json!(1.1)),
            (
                "box_normalized_bottom_left",
                json!({"x":0.9,"y":0.0,"width":0.2,"height":0.1}),
            ),
        ] {
            let mut item = valid.clone();
            item[field] = bad;
            assert!(validate_ocr_items(&json!({"items":[item]})).is_err());
        }
        let mut item = valid.clone();
        item["box_normalized_bottom_left"]["height"] = json!(0);
        assert!(validate_ocr_items(&json!({"items":[item]})).is_err());
        let mut item = valid.clone();
        item.as_object_mut().unwrap().remove("text");
        assert!(validate_ocr_items(&json!({"items":[item]})).is_err());
        assert!(validate_ocr_items(&json!({"items":vec![valid;1001]})).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn image_adapter_accepts_other_recognizers_without_authorizing_input() {
        use std::os::unix::fs::PermissionsExt;
        let img = TestImage::new();
        fs::write(img.path(), PNG).unwrap();
        let hash = image_info(img.path()).unwrap().0;
        let helper = img.path().with_file_name("image-adapter");
        let value = json!({"status":"review_required","action_allowed":false,"image_sha256":hash,
            "image_width":1,"image_height":1,"classification":{"label":"unknown"}});
        fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", value)).unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            recognize_image(img.path(), &helper).unwrap()["classification"]["label"],
            "unknown"
        );
        assert!(recognize_text(img.path(), &helper).is_err());
    }

    #[test]
    fn capped_image_read_rejects_large_and_non_regular_files() {
        let img = TestImage::new();
        fs::write(img.path(), PNG).unwrap();
        assert_eq!(read_image_bounded(img.path()).unwrap(), PNG);
        let file = OpenOptions::new().write(true).open(img.path()).unwrap();
        file.set_len(MAX_IMAGE_BYTES + 1).unwrap();
        assert!(read_image_bounded(img.path())
            .unwrap_err()
            .contains("too large"));
        assert!(read_image_bounded(img.path().parent().unwrap())
            .unwrap_err()
            .contains("regular"));
    }
    #[test]
    fn caller_limit_rejects_oversized_data_before_reading() {
        let img = TestImage::new();
        fs::write(img.path(), PNG).unwrap();
        assert_eq!(
            read_regular_file_bounded(img.path(), PNG.len() as u64).unwrap(),
            PNG
        );
        assert!(read_regular_file_bounded(img.path(), PNG.len() as u64 - 1).is_err());
        assert!(read_regular_file_bounded(img.path(), 0).is_err());
        assert!(read_regular_file_bounded(img.path(), MAX_IMAGE_BYTES + 1).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn capped_image_read_rejects_symbolic_link() {
        use std::os::unix::fs::symlink;
        let img = TestImage::new();
        let target = img.path().with_file_name("target.png");
        fs::write(&target, PNG).unwrap();
        symlink(&target, img.path()).unwrap();
        assert!(read_image_bounded(img.path())
            .unwrap_err()
            .contains("regular"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn windows_only_rejects_legacy_or_unscoped_helper() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TestImage::new();
        let helper = temp.path().with_file_name("ax-helper");
        let write = |scope: &str| {
            let body = json!({"status":"ok","pid":42,"cg_window_binding":"unverified","action_allowed":false,"process_start_source":"kernel_sysctl_kern_proc","tree_scope":scope});
            fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", body)).unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        };
        let backend = MacBackend::new("/unused".into(), helper.clone()).unwrap();
        write("full_bounded_tree");
        assert!(backend.accessibility_windows(42).is_err());
        write("windows_only");
        assert_eq!(
            backend.accessibility_windows(42).unwrap()["tree_scope"],
            "windows_only"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_identity_rejects_legacy_and_wrong_start_sources() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TestImage::new();
        let helper = temp.path().with_file_name("identity-helper");
        let backend = MacBackend::new("/unused".into(), helper.clone()).unwrap();
        let write = |source: Option<&str>| {
            let mut body = json!({"status":"ok","pid":42,"process_start_unix_seconds":123.0,"action_allowed":false});
            if let Some(source) = source {
                body["process_start_source"] = source.into();
            }
            fs::write(&helper, format!("#!/bin/sh\nprintf '%s' '{}'\n", body)).unwrap();
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        };
        write(None);
        assert!(backend.identity(42).is_err());
        write(Some("appkit_launch_date"));
        assert!(backend.identity(42).is_err());
        write(Some("kernel_sysctl_kern_proc"));
        assert!(backend.identity(42).is_ok());
    }

    // Some cases are constructed only by macOS-only tests (PNG decoding is native).
    #[derive(Clone, Copy)]
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    enum CaptureCase {
        Good,
        NoFile,
        NoDigest,
        WrongDigest,
        HeaderOnly,
        NoWidth,
        NoHeight,
        ZeroWidth,
        WrongWidth,
        WrongHeight,
        TooLarge,
        InvalidWidth,
    }
    struct Fake {
        changed: bool,
        process_restarted: bool,
        capture_frame_mismatch: bool,
        capture_case: CaptureCase,
        calls: Cell<u8>,
        identity_calls: Cell<u8>,
    }
    impl Backend for Fake {
        fn identity(&self, pid: i32) -> Result<Value, String> {
            let n = self.identity_calls.get();
            self.identity_calls.set(n + 1);
            Ok(
                json!({"status":"ok", "pid":pid, "process_start_unix_seconds":if self.process_restarted && n > 0 { 456.0 } else { 123.0 }}),
            )
        }
        fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
            let n = self.calls.get();
            self.calls.set(n + 1);
            Ok(vec![Window {
                target: Target::new(42, 7).unwrap(),
                layer: Some(0),
                title: None,
                bounds: Rect {
                    x: if self.changed && n > 0 { 4.0 } else { -20.0 },
                    y: 1.0,
                    width: 100.0,
                    height: 80.0,
                },
            }])
        }
        fn capture(&self, _: Target, out: &Path) -> Result<Value, String> {
            if !matches!(self.capture_case, CaptureCase::NoFile) {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(out)
                    .unwrap();
                file.write_all(if matches!(self.capture_case, CaptureCase::HeaderOnly) {
                    &PNG[..8]
                } else {
                    PNG
                })
                .unwrap();
            }
            let digest = if matches!(
                self.capture_case,
                CaptureCase::NoFile | CaptureCase::WrongDigest | CaptureCase::HeaderOnly
            ) {
                "0".repeat(64)
            } else {
                PNG_SHA256.to_string()
            };
            let mut value = json!({"window_id":7,"owner_pid":42,"image_path":out,
                "window_frame_points":{"x":if self.capture_frame_mismatch { -10.0 } else { -20.0 },
                "y":1.0,"width":100.0,"height":80.0}, "image_sha256":digest,
                "image_width":1,"image_height":1});
            if matches!(self.capture_case, CaptureCase::NoDigest) {
                value.as_object_mut().unwrap().remove("image_sha256");
            }
            match self.capture_case {
                CaptureCase::NoWidth => {
                    value.as_object_mut().unwrap().remove("image_width");
                }
                CaptureCase::NoHeight => {
                    value.as_object_mut().unwrap().remove("image_height");
                }
                CaptureCase::ZeroWidth => value["image_width"] = json!(0),
                CaptureCase::WrongWidth => value["image_width"] = json!(2),
                CaptureCase::WrongHeight => value["image_height"] = json!(2),
                CaptureCase::TooLarge => value["image_width"] = json!(8193),
                CaptureCase::InvalidWidth => value["image_width"] = json!("1"),
                _ => {}
            }
            Ok(value)
        }
    }
    struct AllFake {
        inner: Fake,
        omitted: usize,
        captures: Cell<u32>,
    }
    impl Backend for AllFake {
        fn identity(&self, p: i32) -> Result<Value, String> {
            self.inner.identity(p)
        }
        fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
            panic!("onscreen inventory must not be used")
        }
        fn all_windows(&self, p: i32) -> Result<(Vec<Window>, usize), String> {
            Ok((self.inner.windows(Some(p))?, self.omitted))
        }
        fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
            panic!("onscreen capture must not be used")
        }
        fn capture_all(&self, t: Target, p: &Path) -> Result<Value, String> {
            self.captures.set(self.captures.get() + 1);
            self.inner.capture(t, p)
        }
    }
    fn all_fake(omitted: usize) -> AllFake {
        AllFake {
            inner: Fake {
                changed: false,
                process_restarted: false,
                capture_frame_mismatch: false,
                capture_case: CaptureCase::Good,
                calls: Cell::new(0),
                identity_calls: Cell::new(0),
            },
            omitted,
            captures: Cell::new(0),
        }
    }
    #[test]
    fn all_window_capture_rejects_omitted_inventory_and_preserves_error() {
        let tmp = TestImage::new();
        let f = all_fake(1);
        let result =
            observe_all_with_evidence(&f, Target::new(42, 7).unwrap(), tmp.path()).unwrap();
        assert_eq!(result["status"], "error");
        assert_eq!(result["inventory_scope"], "exact_window_candidate");
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("inventory incomplete"));
        assert_eq!(f.captures.get(), 0);
        assert!(!tmp.path().exists());
        assert!(evidence_path(tmp.path()).unwrap().exists());
        let tmp = TestImage::new();
        let f = all_fake(0);
        let view = AllWindowView::new(&f, Target::new(42, 7).unwrap()).unwrap();
        assert!(view.identity(43).is_err());
        assert!(view.windows(Some(43)).is_err());
        assert!(view
            .capture(Target::new(42, 8).unwrap(), tmp.path())
            .is_err());
        assert_eq!(f.captures.get(), 0);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn all_window_capture_succeeds_with_complete_inventory() {
        let tmp = TestImage::new();
        let f = all_fake(0);
        let result =
            observe_all_with_evidence(&f, Target::new(42, 7).unwrap(), tmp.path()).unwrap();
        assert_eq!(result["status"], "captured");
        assert_eq!(result["action_allowed"], false);
        assert_eq!(f.captures.get(), 1);
        assert!(tmp.path().exists());
    }
    #[test]
    fn unknown_and_non_normal_layers_never_reach_capture() {
        struct LayerBackend {
            layer: Option<i32>,
            captured: Cell<bool>,
        }
        impl Backend for LayerBackend {
            fn identity(&self, pid: i32) -> Result<Value, String> {
                Ok(json!({"status":"ok", "pid":pid, "process_start_unix_seconds":123.0}))
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
                Ok(vec![Window {
                    target: Target::new(42, 7)?,
                    bounds: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 80.0,
                    },
                    layer: self.layer,
                    title: Some("Desktop-like surface".into()),
                }])
            }
            fn capture(&self, _: Target, _: &Path) -> Result<Value, String> {
                self.captured.set(true);
                Err("unexpected capture".into())
            }
        }
        for layer in [None, Some(-2147483603), Some(1), Some(-1)] {
            let b = LayerBackend {
                layer,
                captured: Cell::new(false),
            };
            let observed = b.windows(Some(42)).unwrap().pop().unwrap();
            assert_eq!(
                observed.surface_class(),
                if layer.is_none() {
                    "unknown_layer"
                } else {
                    "non_normal_layer"
                }
            );
            let image = TestImage::new();
            let error = observe(&b, Target::new(42, 7).unwrap(), image.path()).unwrap_err();
            assert!(error.contains("window layer"));
            assert!(!b.captured.get());
            assert!(!image.path().exists());
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn capture_is_unknown_even_when_geometry_matches() {
        let b = Fake {
            changed: false,
            process_restarted: false,
            capture_frame_mismatch: false,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        let image = TestImage::new();
        let out = observe(&b, Target::new(42, 7).unwrap(), image.path()).unwrap();
        assert_eq!(out["decision"], "unknown");
        assert_eq!(out["action_allowed"], false);
        assert_eq!(out["window_layer"], 0);
        assert_eq!(out["surface_class"], "normal_layer_candidate");
    }
    #[test]
    fn rejects_changed_window_and_unsupported_actions() {
        let b = Fake {
            changed: true,
            process_restarted: false,
            capture_frame_mismatch: false,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        let image = TestImage::new();
        assert!(observe(&b, Target::new(42, 7).unwrap(), image.path()).is_err());
        assert!(Target::new(-1, 7).is_err());
        assert_eq!(unsupported_action()["action_allowed"], false);
    }

    #[test]
    fn rejects_process_id_reuse_during_capture() {
        let b = Fake {
            changed: false,
            process_restarted: true,
            capture_frame_mismatch: false,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        let image = TestImage::new();
        assert!(observe(&b, Target::new(42, 7).unwrap(), image.path())
            .unwrap_err()
            .contains("process identity changed"));
    }

    #[test]
    fn rejects_capture_frame_different_from_inventory() {
        let b = Fake {
            changed: false,
            process_restarted: false,
            capture_frame_mismatch: true,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        let image = TestImage::new();
        assert!(observe(&b, Target::new(42, 7).unwrap(), image.path())
            .unwrap_err()
            .contains("capture frame does not match"));
    }

    #[test]
    fn refuses_missing_or_untrusted_image_evidence() {
        for case in [
            CaptureCase::NoFile,
            CaptureCase::NoDigest,
            CaptureCase::WrongDigest,
            CaptureCase::HeaderOnly,
        ] {
            let image = TestImage::new();
            let b = Fake {
                changed: false,
                process_restarted: false,
                capture_frame_mismatch: false,
                capture_case: case,
                calls: Cell::new(0),
                identity_calls: Cell::new(0),
            };
            assert!(observe(&b, Target::new(42, 7).unwrap(), image.path()).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn refuses_dimensions_that_do_not_match_the_decoded_png_and_preserves_evidence() {
        for case in [
            CaptureCase::NoWidth,
            CaptureCase::NoHeight,
            CaptureCase::ZeroWidth,
            CaptureCase::WrongWidth,
            CaptureCase::WrongHeight,
            CaptureCase::TooLarge,
            CaptureCase::InvalidWidth,
        ] {
            let image = TestImage::new();
            let b = Fake {
                changed: false,
                process_restarted: false,
                capture_frame_mismatch: false,
                capture_case: case,
                calls: Cell::new(0),
                identity_calls: Cell::new(0),
            };
            let report =
                observe_with_evidence(&b, Target::new(42, 7).unwrap(), image.path()).unwrap();
            assert_eq!(report["status"], "error");
            assert_eq!(report["action_allowed"], false);
            assert_eq!(report["image_ownership"], "unverified");
            assert!(report["error"]
                .as_str()
                .unwrap()
                .contains("dimensions do not match"));
            assert_eq!(fs::read(image.path()).unwrap(), PNG);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(evidence_path(image.path()).unwrap())
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o077,
                    0
                );
            }
            let saved: Value =
                serde_json::from_slice(&fs::read(evidence_path(image.path()).unwrap()).unwrap())
                    .unwrap();
            assert_eq!(saved, report);
        }
    }

    #[test]
    fn refuses_existing_image_before_calling_backend() {
        let image = TestImage::new();
        fs::write(image.path(), PNG).unwrap();
        let b = Fake {
            changed: false,
            process_restarted: false,
            capture_frame_mismatch: false,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        assert!(observe(&b, Target::new(42, 7).unwrap(), image.path())
            .unwrap_err()
            .contains("already exists"));
        assert_eq!(fs::read(image.path()).unwrap(), PNG);
        assert_eq!(b.identity_calls.get(), 0);
    }

    fn check_record_without_claiming_image(restarted: bool) {
        let image = TestImage::new();
        let b = Fake {
            changed: false,
            process_restarted: restarted,
            capture_frame_mismatch: false,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        let result = observe_with_evidence(&b, Target::new(42, 7).unwrap(), image.path()).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(evidence_path(image.path()).unwrap()).unwrap())
                .unwrap();
        assert_eq!(saved, result);
        assert_eq!(result["action_allowed"], false);
        assert_eq!(fs::read(image.path()).unwrap(), PNG);
        if restarted {
            assert_eq!(result["status"], "error");
            assert_eq!(result["image_ownership"], "unverified");
            assert!(result["error"]
                .as_str()
                .unwrap()
                .contains("process identity changed"));
            assert!(result.get("image_file").is_none());
        } else {
            assert_eq!(result["status"], "captured");
        }
    }

    #[test]
    fn records_changed_process_failure_without_claiming_image() {
        check_record_without_claiming_image(true);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn records_success_without_claiming_image() {
        check_record_without_claiming_image(false);
    }

    #[test]
    fn refuses_existing_sidecar_before_capture_and_preserves_it() {
        let image = TestImage::new();
        let sidecar = evidence_path(image.path()).unwrap();
        fs::write(&sidecar, b"original evidence").unwrap();
        let b = Fake {
            changed: false,
            process_restarted: false,
            capture_frame_mismatch: false,
            capture_case: CaptureCase::Good,
            calls: Cell::new(0),
            identity_calls: Cell::new(0),
        };
        assert!(
            observe_with_evidence(&b, Target::new(42, 7).unwrap(), image.path())
                .unwrap_err()
                .contains("cannot reserve evidence")
        );
        assert_eq!(fs::read(sidecar).unwrap(), b"original evidence");
        assert!(!image.path().exists());
        assert_eq!(b.identity_calls.get(), 0);
    }

    #[test]
    fn preserves_image_created_by_competitor_and_marks_ownership_unknown() {
        struct Race;
        impl Backend for Race {
            fn identity(&self, pid: i32) -> Result<Value, String> {
                Ok(json!({"status":"ok", "pid":pid, "process_start_unix_seconds":123.0}))
            }
            fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
                Ok(vec![Window {
                    target: Target::new(42, 7).unwrap(),
                    layer: Some(0),
                    title: None,
                    bounds: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 10.0,
                        height: 10.0,
                    },
                }])
            }
            fn capture(&self, _: Target, path: &Path) -> Result<Value, String> {
                fs::write(path, b"competing process image").unwrap();
                Err("exclusive create failed: already exists".into())
            }
        }
        let image = TestImage::new();
        let result =
            observe_with_evidence(&Race, Target::new(42, 7).unwrap(), image.path()).unwrap();
        assert_eq!(result["status"], "error");
        assert_eq!(result["image_ownership"], "unverified");
        assert!(result.get("image_sha256").is_none());
        assert!(result.get("image_file").is_none());
        assert_eq!(fs::read(image.path()).unwrap(), b"competing process image");
        let saved: Value =
            serde_json::from_slice(&fs::read(evidence_path(image.path()).unwrap()).unwrap())
                .unwrap();
        assert_eq!(saved, result);
    }

    #[test]
    fn evidence_never_overwrites_an_existing_record() {
        let folder = std::env::temp_dir().join(format!("mac-cu-evidence-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir(&folder).unwrap();
        let image = folder.join("frame.png");
        let first = json!({"decision":"unknown", "action_allowed":false});
        let saved = save_evidence(&image, &first).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&saved).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        assert!(save_evidence(&image, &json!({"decision":"changed"})).is_err());
        let value: Value = serde_json::from_slice(&fs::read(saved).unwrap()).unwrap();
        assert_eq!(value, first);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn refuses_non_png_data_as_capture_evidence() {
        let path = std::env::temp_dir().join(format!("mac-cu-invalid-{}.png", std::process::id()));
        fs::write(&path, b"not a png").unwrap();
        assert!(image_info(&path)
            .unwrap_err()
            .contains("not a supported PNG"));
        fs::remove_file(path).unwrap();
    }

    fn win(window_id: u32, title: Option<&str>, width: f64, height: f64) -> Window {
        Window {
            target: Target::new(42, window_id).unwrap(),
            layer: Some(0),
            title: title.map(str::to_string),
            bounds: Rect {
                x: 10.0,
                y: 20.0,
                width,
                height,
            },
        }
    }

    fn ok_identity() -> Result<Value, String> {
        Ok(json!({"status":"ok", "pid":42, "process_start_unix_seconds":123.0}))
    }

    /// Scripted inventories and identities; the first `windows` call is
    /// "before" and every later call is "after".
    struct ListFake {
        before: Vec<Window>,
        after: Vec<Window>,
        identity: Vec<Result<Value, String>>,
        capture_owner_pid: i32,
        capture_window_id: u32,
        identity_calls: Cell<usize>,
        window_calls: Cell<usize>,
        capture_calls: Cell<usize>,
    }
    impl ListFake {
        fn new(windows: Vec<Window>) -> Self {
            Self {
                before: windows.clone(),
                after: windows,
                identity: vec![ok_identity()],
                capture_owner_pid: 42,
                capture_window_id: 7,
                identity_calls: Cell::new(0),
                window_calls: Cell::new(0),
                capture_calls: Cell::new(0),
            }
        }
    }
    impl Backend for ListFake {
        fn identity(&self, _: i32) -> Result<Value, String> {
            let n = self.identity_calls.get();
            self.identity_calls.set(n + 1);
            self.identity[n.min(self.identity.len() - 1)].clone()
        }
        fn windows(&self, _: Option<i32>) -> Result<Vec<Window>, String> {
            let n = self.window_calls.get();
            self.window_calls.set(n + 1);
            Ok(if n == 0 {
                self.before.clone()
            } else {
                self.after.clone()
            })
        }
        fn capture(&self, _: Target, out: &Path) -> Result<Value, String> {
            self.capture_calls.set(self.capture_calls.get() + 1);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(out)
                .unwrap();
            file.write_all(PNG).unwrap();
            let b = self.before[0].bounds;
            Ok(
                json!({"window_id":self.capture_window_id, "owner_pid":self.capture_owner_pid,
                "image_path":out, "image_sha256":PNG_SHA256,
                "window_frame_points":{"x":b.x,"y":b.y,"width":b.width,"height":b.height},
                "image_width":1, "image_height":1}),
            )
        }
    }
    fn target() -> Target {
        Target::new(42, 7).unwrap()
    }

    #[test]
    fn duplicate_candidates_are_ambiguous() {
        let image = TestImage::new();
        let b = ListFake::new(vec![win(7, None, 100.0, 80.0), win(7, None, 100.0, 80.0)]);
        let err = observe(&b, target(), image.path()).unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");
        assert_eq!(b.capture_calls.get(), 0);
    }

    #[test]
    fn missing_target_is_not_visible() {
        for windows in [vec![], vec![win(8, None, 100.0, 80.0)]] {
            let image = TestImage::new();
            let b = ListFake::new(windows);
            let err = observe(&b, target(), image.path()).unwrap_err();
            assert!(err.contains("not currently visible"), "{err}");
            assert_eq!(b.capture_calls.get(), 0);
        }
    }

    #[test]
    fn invalid_geometry_is_not_a_candidate() {
        for (width, height) in [(0.0, 80.0), (100.0, 0.0), (f64::NAN, 80.0)] {
            let image = TestImage::new();
            let b = ListFake::new(vec![win(7, None, width, height)]);
            let err = observe(&b, target(), image.path()).unwrap_err();
            assert!(err.contains("not currently visible"), "{err}");
            assert_eq!(b.capture_calls.get(), 0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn title_change_does_not_change_identity() {
        let image = TestImage::new();
        let mut b = ListFake::new(vec![win(7, Some("a"), 100.0, 80.0)]);
        b.after = vec![win(7, Some("b"), 100.0, 80.0)];
        let result = observe(&b, target(), image.path()).unwrap();
        assert_eq!(result["window_before"]["title"], "a");
        assert_eq!(result["window_after"]["title"], "b");
        assert_eq!(result["action_allowed"], false);
    }

    #[test]
    fn capture_metadata_owner_or_window_mismatch_is_rejected() {
        for (owner, window) in [(43, 7), (42, 8)] {
            let image = TestImage::new();
            let mut b = ListFake::new(vec![win(7, None, 100.0, 80.0)]);
            b.capture_owner_pid = owner;
            b.capture_window_id = window;
            let err = observe(&b, target(), image.path()).unwrap_err();
            assert!(err.contains("capture metadata does not match"), "{err}");
        }
    }

    #[test]
    fn identity_must_be_ok_and_match_pid() {
        let bad = [
            json!({"status":"error", "pid":42, "process_start_unix_seconds":123.0}),
            json!({"status":"ok", "pid":43, "process_start_unix_seconds":123.0}),
            json!({"status":"ok", "pid":42}),
            json!({"status":"ok", "pid":42, "process_start_unix_seconds":0.0}),
        ];
        for identity in bad {
            let image = TestImage::new();
            let mut b = ListFake::new(vec![win(7, None, 100.0, 80.0)]);
            b.identity = vec![Ok(identity)];
            let err = observe(&b, target(), image.path()).unwrap_err();
            assert!(
                err.contains("process identity is missing or invalid"),
                "{err}"
            );
            assert_eq!(b.window_calls.get(), 0);
        }
    }

    #[test]
    fn second_identity_failure_is_an_error() {
        let image = TestImage::new();
        let mut b = ListFake::new(vec![win(7, None, 100.0, 80.0)]);
        b.identity = vec![ok_identity(), Err("boom".into())];
        let err = observe(&b, target(), image.path()).unwrap_err();
        assert!(err.contains("boom"), "{err}");

        let image = TestImage::new();
        let mut b = ListFake::new(vec![win(7, None, 100.0, 80.0)]);
        b.identity = vec![ok_identity(), Err("boom".into())];
        let result = observe_with_evidence(&b, target(), image.path()).unwrap();
        assert_eq!(result["status"], "error");
        assert_eq!(result["image_ownership"], "unverified");
        assert!(evidence_path(image.path()).unwrap().is_file());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn sidecar_records_before_and_after_windows() {
        let image = TestImage::new();
        let mut b = ListFake::new(vec![win(7, Some("t"), 100.0, 80.0)]);
        b.after = vec![win(7, Some("t"), 100.0, 80.0)];
        observe_with_evidence(&b, target(), image.path()).unwrap();
        let saved: Value =
            serde_json::from_slice(&fs::read(evidence_path(image.path()).unwrap()).unwrap())
                .unwrap();
        assert_eq!(saved["evidence_version"], 2);
        assert_eq!(saved["window_before"]["window_id"], 7);
        assert_eq!(saved["window_before"]["pid"], 42);
        assert_eq!(saved["window_before"]["layer"], 0);
        assert_eq!(saved["window_after"]["bounds_points"]["width"], 100.0);
        assert_eq!(saved["action_allowed"], false);
        assert_eq!(saved["decision"], "unknown");
    }

    #[test]
    fn sidecar_records_failure_with_evidence_version() {
        let failed = ListFake::new(vec![]);
        let image = TestImage::new();
        let report = observe_with_evidence(&failed, target(), image.path()).unwrap();
        assert_eq!(report["status"], "error");
        assert_eq!(report["evidence_version"], 2);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod native_hash_tests {
    #[test]
    fn sha256_standard_vectors() {
        assert_eq!(
            super::sha256_bytes(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            super::sha256_bytes(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            super::sha256_bytes(&vec![b'a'; 1_000_000]).unwrap(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
}
