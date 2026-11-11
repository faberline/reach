//! Offline source checks for the foreground input model (plan I-1). They read
//! files only. Nothing in `src/` may deliver input events, and the model must
//! stay free of FFI, process spawning and event APIs. `cu-agent` may only parse
//! with it (plan I-2); `cu-mcp` never reaches it.
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

/// Plan I-2: `cu-agent` uses the model only to parse an `action` request strictly before
/// it forwards the request to ReachAgent. MCP and the other binaries never use it.
#[test]
fn only_cu_agent_parses_with_the_model_and_mcp_never_does() {
    for rel in [
        "src/bin/cu-agent.rs",
        "src/bin/cu-mcp.rs",
        "src/mcp_server.rs",
        "src/main.rs",
    ] {
        assert!(
            !read(rel).contains("foreground_input"),
            "{rel} must not use the input model"
        );
    }
    let app = read("src/bin/cu-agent/app.rs");
    assert_eq!(app.matches("foreground_input::parse_request(").count(), 1);
    for needle in ["InputGate", "Arming", ".submit(", "LiveFacts"] {
        assert!(
            !app.contains(needle),
            "cu-agent must not run a gate itself: {needle}"
        );
    }
    // The only call that sends `action` is the one ReachAgent call; no helper is spawned for it.
    assert_eq!(app.matches("agent_client::call(").count(), 1);
    assert!(app.contains("agent_client::call(&socket, id, \"action\""));
}

// ---- ReachAgent Swift sources (plan I-2) ----

/// The one Swift file that may name an event delivery API.
const SYSTEM_BACKEND_FILE: &str = "ReachAgentInputSystem.swift";

/// Every Swift file the build scripts compile into ReachAgent, read from the
/// `swiftc` line of each script. Both scripts must compile the same list.
fn reach_agent_swift_files() -> Vec<String> {
    let mut lists = Vec::new();
    for script in ["build-agent.sh", "build-agent-app.sh"] {
        let text = read(script);
        let line = text
            .lines()
            .find(|l| l.trim_start().starts_with("swiftc ") && l.contains("ReachAgent.swift"))
            .unwrap_or_else(|| panic!("{script} has no ReachAgent swiftc line"));
        let files: Vec<String> = line
            .split_whitespace()
            .filter(|t| t.ends_with(".swift"))
            .map(str::to_string)
            .collect();
        lists.push(files);
    }
    assert_eq!(lists[0], lists[1], "build scripts compile different files");
    let files = lists.remove(0);
    assert!(files.contains(&SYSTEM_BACKEND_FILE.to_string()));
    assert!(files.len() >= 5, "too few ReachAgent files: {files:?}");
    files
}

/// Event delivery, AX action and post-event permission request APIs.
const SWIFT_DELIVERY_PATTERNS: &[&str] = &[
    ".post(tap:",
    "postToPid",
    "CGEventPost",
    "CGEvent(",
    "CGEventSource",
    "CGEventTapPostEvent",
    "CGPostKeyboardEvent",
    "CGPostMouseEvent",
    "AXUIElementPerformAction",
    "AXUIElementPostKeyboardEvent",
    "AXUIElementSetAttributeValue",
    "CGRequestPostEventAccess",
    "IOHIDPostEvent",
    "NSAppleScript",
];

#[test]
fn only_the_system_backend_file_names_event_delivery_apis() {
    let mut problems = Vec::new();
    for file in reach_agent_swift_files() {
        if file == SYSTEM_BACKEND_FILE {
            continue;
        }
        let text = read(&file);
        for needle in SWIFT_DELIVERY_PATTERNS {
            if text.contains(needle) {
                problems.push(format!("{file}: {needle:?}"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "event delivery API outside {SYSTEM_BACKEND_FILE}:\n{}",
        problems.join("\n")
    );
    // The system file really is the delivery file, so the scan is not vacuous.
    let system = read(SYSTEM_BACKEND_FILE);
    for needle in [".post(tap:", "CGEvent(", "CGRequestPostEventAccess"] {
        assert!(
            system.contains(needle),
            "{SYSTEM_BACKEND_FILE} lacks {needle}"
        );
    }
}

#[test]
fn the_system_backend_has_no_forbidden_input_kinds() {
    let system = read(SYSTEM_BACKEND_FILE);
    for needle in [
        "scrollWheel",
        "Dragged",
        "rightMouse",
        "otherMouse",
        "maskCommand",
        "maskShift",
        "maskAlternate",
        "maskControl",
        ".activate(",
        "postToPid",
        "AXUIElement",
        "com.google.Chrome",
        "NSAppleScript",
    ] {
        assert!(
            !system.contains(needle),
            "{SYSTEM_BACKEND_FILE} mentions {needle}"
        );
    }
    assert!(
        system.contains("fileprivate init()"),
        "SystemInputLaunch must keep a fileprivate initializer"
    );
}

/// The system backend is reachable only through `SystemInputLaunch.authorize`,
/// which refuses `--test-mode`, a non-empty `CI` and a launch without input.
#[test]
fn the_system_backend_is_reached_only_through_the_startup_guard() {
    let system = read(SYSTEM_BACKEND_FILE);
    let authorize = system
        .split("static func authorize(")
        .nth(1)
        .expect("authorize exists")
        .split("\n    }\n")
        .next()
        .unwrap();
    assert!(authorize.contains("guard enableInput, !testMode else { return nil }"));
    assert!(authorize.contains("environment[\"CI\"]"));
    assert_eq!(system.matches("SystemInputLaunch()").count(), 1);
    assert!(system.contains("func makeSystemInputBackend(_ launch: SystemInputLaunch)"));
    assert!(system.contains("func requestPostEventAccess(_ launch: SystemInputLaunch)"));

    for file in reach_agent_swift_files() {
        if file == SYSTEM_BACKEND_FILE {
            continue;
        }
        let text = read(&file);
        assert!(
            !text.contains("SystemInputLaunch()"),
            "{file} constructs SystemInputLaunch directly"
        );
        let expected = usize::from(file == "ReachAgent.swift");
        assert_eq!(
            text.matches("makeSystemInputBackend(").count(),
            expected,
            "{file}: makeSystemInputBackend calls"
        );
        assert_eq!(
            text.matches("requestPostEventAccess(").count(),
            expected,
            "{file}: requestPostEventAccess calls"
        );
    }
    // `authorize` reads the process environment itself; no caller can hand it one.
    assert!(authorize.contains("ProcessInfo.processInfo.environment[\"CI\"]"));
    assert!(system.contains(
        "static func authorize(enableInput: Bool, testMode: Bool) -> SystemInputLaunch?"
    ));
    let agent = read("ReachAgent.swift");
    assert!(agent.contains("input_unavailable"));
}

/// Every call site of the start-up guard and of `makeInputController`, as exact lines.
/// This is a guard against mistakes, not a security boundary: it fails when a new call
/// appears (for example a self-test that reaches the system path with `testMode: false`).
#[test]
fn the_startup_guard_and_input_controller_have_only_the_known_call_sites() {
    let expected: &[(&str, &[&str])] = &[
        (
            "SystemInputLaunch.authorize(",
            &[
                "ReachAgent.swift: check(SystemInputLaunch.authorize(enableInput: true, testMode: true) == nil,",
                "ReachAgent.swift: check(SystemInputLaunch.authorize(enableInput: false, testMode: false) == nil,",
                "ReachAgent.swift: guard let launch = SystemInputLaunch.authorize(enableInput: true, testMode: false) else {",
            ],
        ),
        (
            "makeInputController(",
            &[
                "ReachAgent.swift: let made = try? makeInputController(testConfig)",
                "ReachAgent.swift: (try? makeInputController(ServeConfig(socket: \"/tmp/x\", testMode: true, input: nil)))",
                "ReachAgent.swift: func makeInputController(_ config: ServeConfig) throws -> (",
                "ReachAgent.swift: let input = try makeInputController(config)",
            ],
        ),
    ];
    let files = reach_agent_swift_files();
    for (needle, lines) in expected {
        let mut found = Vec::new();
        for file in &files {
            for line in read(file)
                .lines()
                .filter(|l| l.contains(needle) && !l.trim_start().starts_with("//"))
            {
                found.push(format!("{file}: {}", line.trim()));
            }
        }
        assert_eq!(found, *lines, "call sites of {needle}");
    }
}

/// The Rust model and the Swift agent accept the same six keycodes (plan 1.3, Q10).
#[test]
fn rust_and_swift_approve_the_same_keycodes() {
    let swift = read("ReachAgentInput.swift");
    let line = swift
        .lines()
        .find(|l| l.starts_with("let approvedInputKeycodes = ["))
        .expect("approvedInputKeycodes exists");
    let list: Vec<u16> = line
        .split('[')
        .nth(1)
        .and_then(|s| s.split(']').next())
        .unwrap()
        .split(',')
        .map(|n| n.trim().parse().unwrap())
        .collect();
    assert_eq!(list, mac_computer_use::foreground_input::APPROVED_KEYCODES);
    for modifier in 54..=63 {
        assert!(!list.contains(&modifier), "modifier {modifier} is approved");
    }
}
