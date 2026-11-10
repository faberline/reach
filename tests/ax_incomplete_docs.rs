//! The joint observation document must list how incomplete AX data is reported.
//! Offline: only reads files.
use std::{fs, path::PathBuf};

#[test]
fn ax_incomplete_doc_lists_reasons() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/JOINT-OBSERVATION.md");
    let doc = fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
    for needle in [
        "## AX 資料不完整時的回報",
        "ax_snapshot_incomplete_or_unsafe",
        "ax_window_fields_unverified",
        "process_identity_unverified_or_changed",
        "raw_exact_matches",
        "ax_field_diagnostics",
        "candidate_count",
        "observation_failed",
        "cg_window_binding",
    ] {
        assert!(
            doc.contains(needle),
            "JOINT-OBSERVATION.md lacks {needle:?}"
        );
    }
}
