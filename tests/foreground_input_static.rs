//! Offline source checks for the foreground input model (plan I-1). They read
//! files only. The model must stay free of any event-delivery API, FFI, and
//! process spawning; `cu-agent` and `cu-mcp` must not reach it yet.

use std::{fs, path::PathBuf};

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn model_sources_call_no_delivery_api_ffi_or_process() {
    let forbidden = [
        "CGEventPost",
        "CGEventCreate",
        "CGEventTap",
        "postToPid",
        "CGPostKeyboardEvent",
        "CGPostMouseEvent",
        "AXUIElementPerformAction",
        "AXUIElementPostKeyboardEvent",
        "extern ",
        "unsafe",
        "std::process",
        "Command::new",
        "std::fs",
        "std::net",
    ];
    for rel in [
        "src/foreground_input.rs",
        "src/foreground_input/tests.rs",
        "src/input_guard.rs",
    ] {
        let text = read(rel);
        for needle in forbidden {
            assert!(!text.contains(needle), "{rel} mentions {needle}");
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
