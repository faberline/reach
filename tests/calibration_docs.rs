//! The calibration document must state its reasons and limits and be reachable.
//! Offline: only reads files.
use std::{fs, path::PathBuf};

fn read(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[test]
fn calibration_doc_states_reasons_and_limits() {
    let doc = read("docs/CALIBRATION.md");
    for needle in [
        "markers_insufficient",
        "markers_collinear",
        "frame_origin_mismatch",
        "residual_exceeds_threshold",
        "display_changed",
        "window_frame_changed",
        "scale_changed",
        "calibration_expired",
        "MAX_CALIBRATION_AGE",
        "evidence_id",
        "action_allowed",
        "evidence_hash_unavailable",
    ] {
        assert!(doc.contains(needle), "CALIBRATION.md lacks {needle:?}");
    }
}

#[test]
fn calibration_doc_is_linked() {
    for path in ["README.md", "docs/CAPABILITIES.md"] {
        assert!(
            read(path).contains("CALIBRATION.md"),
            "{path} does not link CALIBRATION.md"
        );
    }
}
