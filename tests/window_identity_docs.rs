//! The window identity document must state its limits and be reachable.
//! Offline: only reads files.
use std::{fs, path::PathBuf};

fn read(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

#[test]
fn window_identity_doc_states_limits() {
    let doc = read("docs/WINDOW-IDENTITY.md");
    for needle in [
        "遮擋",
        "畫面新鮮度",
        "非原子",
        "螢幕比例",
        "evidence_version",
        "CGPreflightScreenCaptureAccess",
    ] {
        assert!(doc.contains(needle), "WINDOW-IDENTITY.md lacks {needle:?}");
    }
}

#[test]
fn window_identity_doc_is_linked() {
    for path in ["README.md", "docs/CAPABILITIES.md"] {
        assert!(
            read(path).contains("WINDOW-IDENTITY.md"),
            "{path} does not link WINDOW-IDENTITY.md"
        );
    }
}

#[test]
fn window_identity_doc_matches_evidence_fields() {
    let doc = read("docs/WINDOW-IDENTITY.md");
    for needle in [
        "<圖片檔名>.json",
        "new.png.json",
        "requested_image_path",
        "image_ownership",
        "evidence_file",
        "座標有限",
        "寬高為正",
        "計算重複候選之前",
        "#16",
        "不會留下新的 sidecar",
        "stream_status",
        "沒有 `title`",
    ] {
        assert!(doc.contains(needle), "WINDOW-IDENTITY.md lacks {needle:?}");
    }
    for stale in ["與圖片同名", "幾何為有限正值"] {
        assert!(
            !doc.contains(stale),
            "WINDOW-IDENTITY.md still says {stale:?}"
        );
    }
}

#[test]
fn window_identity_doc_fields_exist_in_source() {
    let lib = read("src/lib.rs");
    for needle in [
        "\"requested_image_path\"",
        "\"image_ownership\"",
        "\"evidence_file\"",
        "\"window_before\"",
        "\"bounds_points\"",
        "format!(\"{filename}.json\")",
    ] {
        assert!(lib.contains(needle), "src/lib.rs lacks {needle:?}");
    }
    let status = read("src/platform/macos/stream_status.rs");
    assert!(status.contains("\"bounds\""), "stream_status lacks bounds");
    assert!(
        !status.contains("\"title\""),
        "stream_status window_json now has a title; update WINDOW-IDENTITY.md"
    );
}
