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

/// Swift source without `//` comment lines (doc comments included), so a comment that names
/// a call does not count as one. Backticks are removed too, so `` `authorize` `` reads as
/// `authorize` (plan review P31-2).
fn swift_code(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
        .replace('`', "")
}

/// `typealias X = SystemInputLaunch` in any spelling of whitespace.
fn aliases_system_input_launch(code: &str) -> bool {
    let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    compact.split("typealias").skip(1).any(|stmt| {
        stmt.split_once('=')
            .is_some_and(|(_, rhs)| rhs.starts_with("SystemInputLaunch"))
    })
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every call of `name` in `code`, matched as a token like the regex `\bname\s*\(`: the name
/// has no identifier character before or after it, and only whitespace (newlines included)
/// stands between it and `(`. Each hit is reported as its trimmed source line, so a call split
/// over two lines or written with a space still shows up, and differs from the known line.
fn call_sites(code: &str, name: &str) -> Vec<String> {
    token_sites(code, name, true)
}

/// Like `call_sites`, but with `calls_only` false every use of the identifier counts.
fn token_sites(code: &str, name: &str, calls_only: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (start, _) in code.match_indices(name) {
        let end = start + name.len();
        if code[..start].chars().next_back().is_some_and(is_ident)
            || code[end..].chars().next().is_some_and(is_ident)
        {
            continue;
        }
        if calls_only && !code[end..].trim_start().starts_with('(') {
            continue;
        }
        let line_start = code[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = code[end..].find('\n').map_or(code.len(), |i| end + i);
        out.push(code[line_start..line_end].trim().to_string());
    }
    out
}

/// `extension SystemInputLaunch` with any whitespace between the two tokens.
fn extends_system_input_launch(code: &str) -> bool {
    code.split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0] == "extension" && w[1].split(['{', ':']).next() == Some("SystemInputLaunch"))
}

/// Every call site of the start-up guard and of `makeInputController`, as exact lines, and
/// every use of `SystemInputLaunch` outside the system file (plan review N1). This is a guard
/// against mistakes, not a security boundary: it fails when a new call appears (for example a
/// self-test that reaches the system path with `testMode: false`), including one split across
/// lines, written with a space before `(`, or placed in an extension.
#[test]
fn the_startup_guard_and_input_controller_have_only_the_known_call_sites() {
    let expected: &[(&str, &[&str])] = &[
        (
            "authorize",
            &[
                "ReachAgent.swift: check(SystemInputLaunch.authorize(enableInput: true, testMode: true) == nil,",
                "ReachAgent.swift: check(SystemInputLaunch.authorize(enableInput: false, testMode: false) == nil,",
                "ReachAgent.swift: guard let launch = SystemInputLaunch.authorize(enableInput: true, testMode: false) else {",
                "ReachAgentInputSystem.swift: static func authorize(enableInput: Bool, testMode: Bool) -> SystemInputLaunch? {",
            ],
        ),
        (
            "makeInputController",
            &[
                "ReachAgent.swift: let made = try? makeInputController(testConfig)",
                "ReachAgent.swift: (try? makeInputController(ServeConfig(socket: \"/tmp/x\", testMode: true, input: nil)))",
                "ReachAgent.swift: func makeInputController(_ config: ServeConfig) throws -> (",
                "ReachAgent.swift: let input = try makeInputController(config)",
            ],
        ),
    ];
    let files = reach_agent_swift_files();
    for (name, lines) in expected {
        let mut found = Vec::new();
        for file in &files {
            // Every use counts, not only calls: `let f = makeInputController` is a use too.
            for line in token_sites(&swift_code(&read(file)), name, false) {
                found.push(format!("{file}: {line}"));
            }
        }
        assert_eq!(found, *lines, "call sites of {name}");
    }

    // Outside the system file the type may appear only in these lines: no extension, no
    // typealias, no stored reference to `authorize`.
    let mut found = Vec::new();
    for file in &files {
        let code = swift_code(&read(file));
        assert!(
            !aliases_system_input_launch(&code),
            "{file} aliases SystemInputLaunch"
        );
        assert!(
            !extends_system_input_launch(&code),
            "{file} extends SystemInputLaunch"
        );
        for line in token_sites(&code, "SystemInputLaunch", false) {
            found.push(format!("{file}: {line}"));
        }
    }
    assert_eq!(
        found,
        [
            "ReachAgent.swift: var inputLaunch: SystemInputLaunch? = nil",
            "ReachAgent.swift: check(SystemInputLaunch.authorize(enableInput: true, testMode: true) == nil,",
            "ReachAgent.swift: check(SystemInputLaunch.authorize(enableInput: false, testMode: false) == nil,",
            "ReachAgent.swift: controller: InputController?, launch: SystemInputLaunch?",
            "ReachAgent.swift: guard let launch = SystemInputLaunch.authorize(enableInput: true, testMode: false) else {",
            "ReachAgentInputSystem.swift: struct SystemInputLaunch {",
            "ReachAgentInputSystem.swift: static func authorize(enableInput: Bool, testMode: Bool) -> SystemInputLaunch? {",
            "ReachAgentInputSystem.swift: return SystemInputLaunch()",
            "ReachAgentInputSystem.swift: func requestPostEventAccess(_ launch: SystemInputLaunch) {",
            "ReachAgentInputSystem.swift: func makeSystemInputBackend(_ launch: SystemInputLaunch) -> (sampler: InputSampler, poster: EventPoster) {",
        ],
        "uses of SystemInputLaunch"
    );
}

/// The call-site scan catches the bypasses that an exact-line `contains` missed (plan review N1).
#[test]
fn the_call_site_scan_catches_the_known_bypasses() {
    let known =
        "guard let launch = SystemInputLaunch.authorize(enableInput: true, testMode: false) else {";
    assert_eq!(call_sites(known, "authorize"), [known]);
    // (a) A second call hidden in an extension.
    let a = "extension SystemInputLaunch {\n    static func sneak() -> SystemInputLaunch? {\n        authorize(enableInput: true, testMode: false)\n    }\n}\n";
    assert_eq!(
        call_sites(a, "authorize"),
        ["authorize(enableInput: true, testMode: false)"]
    );
    assert!(extends_system_input_launch(a));
    assert!(extends_system_input_launch(
        "extension\n  SystemInputLaunch{ }"
    ));
    assert!(extends_system_input_launch(
        "extension SystemInputLaunch: Equatable {}"
    ));
    assert!(!extends_system_input_launch(
        "extension SystemInputLaunchX {}"
    ));
    // (b) The type and the call on two lines.
    let b = "let l = SystemInputLaunch\n    .authorize(enableInput: true, testMode: false)\n";
    let hits = call_sites(b, "authorize");
    assert_eq!(hits, [".authorize(enableInput: true, testMode: false)"]);
    assert_ne!(hits[0], known);
    assert_eq!(token_sites(b, "SystemInputLaunch", false).len(), 1);
    // (c) A space, or a newline, before the parenthesis.
    assert_eq!(
        call_sites(
            "let x = try makeInputController (config)",
            "makeInputController"
        ),
        ["let x = try makeInputController (config)"]
    );
    assert_eq!(
        call_sites(
            "let x = try makeInputController\n(config)",
            "makeInputController"
        )
        .len(),
        1
    );
    // A stored reference is not a call, but it is a use of the type.
    let r = "let f = SystemInputLaunch.authorize\n";
    assert!(call_sites(r, "authorize").is_empty());
    assert_eq!(token_sites(r, "SystemInputLaunch", false).len(), 1);
    // P31-2 (d): backticks around the name.
    let d = swift_code("SystemInputLaunch.`authorize`(enableInput: true, testMode: false)");
    assert_eq!(call_sites(&d, "authorize").len(), 1);
    // (e) A stored function reference: only an every-use scan sees it.
    let e = "let f = SystemInputLaunch.authorize\nf(true, false)\n";
    assert!(call_sites(e, "authorize").is_empty());
    assert_eq!(token_sites(e, "authorize", false).len(), 1);
    // (f) A typealias of the type, from any file.
    assert!(aliases_system_input_launch(
        "typealias ProbeL = SystemInputLaunch"
    ));
    assert!(aliases_system_input_launch(
        "typealias  ProbeL\n = SystemInputLaunch\n"
    ));
    assert!(!aliases_system_input_launch("typealias A = Int"));
    // (h) Backticks around `makeInputController`.
    let h =
        swift_code("`makeInputController`(ServeConfig(socket: s, testMode: false, input: nil))");
    assert_eq!(token_sites(&h, "makeInputController", false).len(), 1);
    // (i) A stored reference to `makeInputController`.
    let i = "let f = makeInputController\nf(config)\n";
    assert!(call_sites(i, "makeInputController").is_empty());
    assert_eq!(token_sites(i, "makeInputController", false).len(), 1);
    // Word boundaries: longer identifiers are not hits, and comment lines are dropped.
    assert!(call_sites("reauthorize(x) authorizeAll(y)", "authorize").is_empty());
    let comments = swift_code("// authorize(enableInput: true, testMode: false)\n/// authorize(x)");
    assert!(call_sites(&comments, "authorize").is_empty());
}

/// APIs that move the pointer, build events outside the system backend's two helpers, run
/// scripts or other processes, or bring an app to the front (plan review R4). None is allowed
/// in any ReachAgent file, the system backend included. Whitespace is removed before the
/// substring scan, so `. activate (` is caught too.
const SWIFT_FORBIDDEN_EVERYWHERE: &[&str] = &[
    "CGWarpMouseCursorPosition",
    "CGDisplayMoveCursorToPoint",
    "CGEvent.init",
    "osascript",
    "NSTask",
    ".activate(",
    "activateIgnoringOtherApps",
    "NSAppleScript",
    "NSUserAppleScriptTask",
    "Process.launchedProcess",
    "posix_spawn",
    "system(",
    ".open(",
    "openApplication",
];

fn forbidden_everywhere_hits(code: &str) -> Vec<String> {
    let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let mut hits: Vec<String> = SWIFT_FORBIDDEN_EVERYWHERE
        .iter()
        .filter(|n| compact.contains(**n))
        .map(|n| n.to_string())
        .collect();
    hits.extend(call_sites(code, "Process"));
    hits
}

#[test]
fn no_reach_agent_file_moves_the_pointer_spawns_or_activates() {
    let mut problems = Vec::new();
    for file in reach_agent_swift_files() {
        for hit in forbidden_everywhere_hits(&swift_code(&read(&file))) {
            problems.push(format!("{file}: {hit}"));
        }
    }
    assert!(
        problems.is_empty(),
        "forbidden in ReachAgent:\n{}",
        problems.join("\n")
    );
    // The scan itself recognizes the spellings it exists to stop.
    for sample in [
        "CGWarpMouseCursorPosition(p)",
        "CGDisplayMoveCursorToPoint(d, p)",
        "let e = CGEvent.init(source: nil)",
        "let e = CGEvent\n    .init(source: nil)",
        "p.launchPath = \"/usr/bin/osascript\"",
        "let t = NSTask()",
        "let p = Process()",
        "let p = Process ()",
        "app.activate(options: [])",
        "NSApp\n  .activate (ignoringOtherApps: true)",
        "app.activateIgnoringOtherApps(true)",
        "let p = Process.launchedProcess(launchPath: a, arguments: [])",
        "posix_spawn(&pid, path, nil, nil, argv, nil)",
        "system(\"ls\")",
        "NSWorkspace.shared.open(url)",
        "NSWorkspace.shared\n  .open (url)",
        "NSWorkspace.shared.openApplication(at: u, configuration: c)",
    ] {
        assert!(
            !forbidden_everywhere_hits(sample).is_empty(),
            "the scan misses {sample:?}"
        );
    }
    for sample in [
        "ProcessInfo.processInfo.environment",
        "kernelStart(pid)",
        "var p: Process?",
        "let app = NSWorkspace.shared.frontmostApplication",
        "fileSystem(x)",
    ] {
        assert!(
            forbidden_everywhere_hits(sample).is_empty(),
            "the scan flags {sample:?}"
        );
    }
}

/// The Swift agent and the Rust model use the same title-bar band and modifier mask
/// (plan review R1 and R2).
#[test]
fn rust_and_swift_share_the_title_band_and_modifier_mask() {
    use mac_computer_use::foreground_input::{HELD_MODIFIER_FLAGS_MASK, TITLE_BAR_BAND_POINTS};
    let swift = read("ReachAgentInput.swift");
    let value = |prefix: &str| -> String {
        swift
            .lines()
            .find_map(|l| l.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("ReachAgentInput.swift lacks {prefix:?}"))
            .trim()
            .to_string()
    };
    let band: f64 = value("let titleBarBandPoints = ").parse().unwrap();
    assert_eq!(band, TITLE_BAR_BAND_POINTS);
    let mask = value("let heldModifierFlagsMask: UInt64 = ")
        .split('|')
        .map(|t| {
            u64::from_str_radix(&t.trim().trim_start_matches("0x").replace('_', ""), 16).unwrap()
        })
        .fold(0, |a, b| a | b);
    assert_eq!(mask, HELD_MODIFIER_FLAGS_MASK);
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
