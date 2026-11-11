//! Offline source checks for the foreground input model (plan I-1). They read
//! files only. Nothing in `src/` may deliver input events, and the model must
//! stay free of FFI, process spawning and event APIs; `cu-agent` and `cu-mcp`
//! must not reach it yet.
//!
//! The scan covers every `.rs` file under `src/`, so a new module cannot slip
//! past it. A pattern is allowed only in files named in an explicit list below;
//! a file that is not listed has no allowance at all (new files included).

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every `src/**/*.rs` as (path relative to the crate root, text).
fn all_sources() -> Vec<(String, String)> {
    let mut paths = Vec::new();
    rust_sources(&root().join("src"), &mut paths);
    assert!(
        paths.len() > 20,
        "scan found too few sources: {}",
        paths.len()
    );
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(&p).unwrap();
            (rel, text)
        })
        .collect()
}

/// Files that mention a pattern today, and why that is not a delivery path.
/// Pattern list entries are plain substrings.
struct Rule {
    why: &'static str,
    patterns: &'static [&'static str],
    allowed_files: &'static [&'static str],
}

const RULES: &[Rule] = &[
    // Event delivery entry points. Only guard tests may name them, as strings
    // that the guards assert are absent from other code.
    Rule {
        why: "event delivery API",
        patterns: &[
            "CGEventPost",
            "CGEventTapPostEvent",
            "CGPostKeyboardEvent",
            "CGPostMouseEvent",
            "postToPid",
            "IOHIDPostEvent",
            "AXUIElementPerformAction",
            "AXUIElementPostKeyboardEvent",
            "AXUIElementPostKeyboardEvent",
        ],
        allowed_files: &["src/agent_protocol.rs"],
    },
    // Any CGEvent* symbol. `native_event_preview.rs` only creates and inspects
    // events in memory (it declares no post function); the other two list the
    // names as strings in guard tests.
    Rule {
        why: "CGEvent family",
        patterns: &["CGEvent"],
        allowed_files: &[
            "src/agent_protocol.rs",
            "src/cursor_overlay.rs",
            "src/native_event_preview.rs",
        ],
    },
    Rule {
        why: "IOHID family",
        patterns: &["IOHID"],
        allowed_files: &["src/cursor_overlay.rs"],
    },
    // Binding crates for the same APIs, or crates that synthesize input.
    Rule {
        why: "event or binding crates",
        patterns: &[
            "core_graphics::",
            "core-graphics",
            "core_foundation",
            "core-foundation",
            "objc",
            "cocoa::",
            "enigo",
            "rdev",
            "inputbot",
            "autopilot",
            "libc::",
            "mach2",
            "io_kit",
        ],
        allowed_files: &[],
    },
    // Hand-written FFI. Each listed file existed before the foreground model;
    // they declare capture, window, AX and display calls.
    Rule {
        why: "FFI declarations",
        patterns: &["#[link", "extern \"C\"", "extern {", "unsafe"],
        allowed_files: &[
            "src/ax_cg.rs",
            "src/bin/cu-agent/app.rs",
            "src/bin/cu-focus-watch.rs",
            "src/bin/cu-headless-check.rs",
            "src/bin/cu-hidden-appkit-check.rs",
            "src/bin/cu-hidden-window-check.rs",
            "src/bounded_process.rs",
            "src/display_inventory.rs",
            "src/live_ax_cg.rs",
            "src/native_event_preview.rs",
            "src/platform/macos/backend.rs",
            "src/platform/macos/capture_lock.rs",
            "src/platform/macos/codec.rs",
            "src/platform/macos/stream_image.rs",
            "src/platform/macos/windows.rs",
        ],
    },
    // Process spawning and process-level helpers (`process::Command`,
    // `use std::{process::Command}`, `std::process::id`, ...).
    Rule {
        why: "process spawning",
        patterns: &["process::", "Command"],
        allowed_files: &[
            "src/agent_client.rs",
            "src/agent_protocol.rs",
            "src/bin/cu-agent.rs",
            "src/bin/cu-agent/app.rs",
            "src/bin/cu-focus-watch.rs",
            "src/bin/cu-headless-check.rs",
            "src/bin/cu-hidden-appkit-check.rs",
            "src/bin/cu-hidden-window-check.rs",
            "src/bin/cu-mcp.rs",
            "src/bin/display-inventory.rs",
            "src/bounded_process.rs",
            "src/cursor_overlay.rs",
            "src/joint_observation.rs",
            "src/lib.rs",
            "src/main.rs",
            "src/mcp_server.rs",
            "src/overlay_session.rs",
            "src/platform/macos/backend.rs",
            "src/platform/macos/capture_lock.rs",
            "src/platform/macos/stream_image.rs",
            "src/platform/macos/stream_status.rs",
        ],
    },
];

#[test]
fn no_source_under_src_delivers_input_or_adds_ffi_or_process_use() {
    let sources = all_sources();
    let mut seen_allowed: BTreeSet<&str> = BTreeSet::new();
    let mut problems = Vec::new();
    for rule in RULES {
        for allowed in rule.allowed_files {
            assert!(
                sources.iter().any(|(rel, _)| rel == allowed),
                "allowlist names a missing file {allowed} ({})",
                rule.why
            );
            seen_allowed.insert(allowed);
        }
        for (rel, text) in &sources {
            if rule.allowed_files.contains(&rel.as_str()) {
                continue;
            }
            for needle in rule.patterns {
                if text.contains(needle) {
                    problems.push(format!("{rel}: {} ({needle:?})", rule.why));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "unlisted use found; delivery code needs plan I-2 and a deliberate allowlist entry:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_model_sources_have_no_allowance_for_anything() {
    for rel in [
        "src/foreground_input.rs",
        "src/foreground_input/tests.rs",
        "src/input_guard.rs",
    ] {
        for rule in RULES {
            assert!(
                !rule.allowed_files.contains(&rel),
                "{rel} must not be on any allowlist ({})",
                rule.why
            );
        }
    }
    // Also forbidden in the model for reasons that are not about delivery.
    for rel in [
        "src/foreground_input.rs",
        "src/foreground_input/tests.rs",
        "src/input_guard.rs",
    ] {
        let text = read(rel);
        for needle in ["std::fs", "std::net", "std::os::"] {
            assert!(!text.contains(needle), "{rel} mentions {needle}");
        }
    }
}

#[test]
fn the_scan_recognizes_the_evasions_it_exists_to_stop() {
    // The patterns above must match these spellings; this guards the list itself.
    let cases = [
        ("use std::{process::Command};", "process::"),
        (
            "core_graphics::event::CGEvent::post(tap)",
            "core_graphics::",
        ),
        ("#[link(name = \"ApplicationServices\")]", "#[link"),
        ("unsafe extern \"C\" { fn f(); }", "extern \"C\""),
        ("CGEventPost(0, e)", "CGEventPost"),
        ("IOHIDPostEvent(c, 1)", "IOHIDPostEvent"),
        ("use core_foundation::base::TCFType;", "core_foundation"),
    ];
    for (sample, expected) in cases {
        let hit = RULES
            .iter()
            .flat_map(|r| r.patterns.iter())
            .any(|p| sample.contains(p) && *p == expected);
        assert!(hit, "no rule pattern {expected:?} matches {sample:?}");
    }
}

const FORBIDDEN_CRATES: &[&str] = &[
    "core-graphics",
    "core-graphics-types",
    "core-foundation",
    "core-foundation-sys",
    "objc",
    "objc2",
    "objc-sys",
    "cocoa",
    "cocoa-foundation",
    "libc",
    "mach2",
    "io-kit-sys",
    "enigo",
    "rdev",
    "inputbot",
    "autopilot",
    "evdev",
    "uinput",
    "device_query",
    "accessibility",
    "accessibility-sys",
];

/// The manifest names every dependency; none but serde and serde_json exist.
#[test]
fn cargo_manifest_and_lockfile_have_no_event_or_ffi_crates() {
    let manifest = read("Cargo.toml");
    let mut in_deps = false;
    let mut names = Vec::new();
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t.contains("dependencies");
            assert!(
                !t.contains("build-dependencies"),
                "a build script could link anything: {t}"
            );
            continue;
        }
        if in_deps && !t.is_empty() && !t.starts_with('#') {
            if let Some((name, _)) = t.split_once('=') {
                names.push(name.trim().trim_matches('"').to_string());
            }
        }
    }
    assert!(!names.is_empty(), "manifest parse found no dependencies");
    for name in &names {
        assert!(
            matches!(name.as_str(), "serde" | "serde_json"),
            "new dependency {name:?}: adding one needs a deliberate update of this test"
        );
        assert!(!FORBIDDEN_CRATES.contains(&name.as_str()));
    }
    assert!(
        !root().join("build.rs").exists(),
        "no build script is allowed"
    );

    let lock = read("Cargo.lock");
    for line in lock.lines() {
        if let Some(name) = line.trim().strip_prefix("name = ") {
            let name = name.trim_matches('"');
            assert!(
                !FORBIDDEN_CRATES.contains(&name),
                "Cargo.lock contains forbidden crate {name:?}"
            );
        }
    }
}

#[test]
fn agent_and_mcp_do_not_use_the_model_yet() {
    for rel in [
        "src/bin/cu-agent.rs",
        "src/bin/cu-agent/app.rs",
        "src/bin/cu-mcp.rs",
        "src/mcp_server.rs",
        "src/main.rs",
    ] {
        assert!(
            !read(rel).contains("foreground_input"),
            "{rel} must not use the offline model before plan I-2"
        );
    }
}
