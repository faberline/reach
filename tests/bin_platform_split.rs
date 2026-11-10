//! The two macOS-only bins must keep their macOS code behind cfg so they also
//! compile on other targets. Offline: only reads source files.
use std::{fs, path::PathBuf};

fn read(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

const MESSAGE: &str = "unsupported platform: only macOS is supported";

#[test]
fn cu_agent_entry_gates_macos_app() {
    let entry = read("src/bin/cu-agent.rs");
    assert!(
        entry.contains("#[cfg(target_os = \"macos\")]\n#[path = \"cu-agent/app.rs\"]\nmod app;"),
        "entry must gate `mod app;` on macOS and point at cu-agent/app.rs"
    );
    assert!(entry.contains("#[cfg(not(target_os = \"macos\"))]"));
    for forbidden in ["MacBackend", "stream_image", "recognize_"] {
        assert!(!entry.contains(forbidden), "entry mentions {forbidden}");
    }
    assert!(entry.lines().count() < 80, "entry must stay thin");
    assert!(
        read("src/bin/cu-agent/app.rs").contains("pub fn main() -> ExitCode"),
        "app.rs must expose main"
    );
}

#[test]
fn main_gates_macos_backend() {
    let main = read("src/main.rs");
    assert!(main.contains("#[cfg(not(target_os = \"macos\"))]"));
    let lines: Vec<&str> = main.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("MacBackend") {
            // The nearest `#[cfg(..)]` above must be the macOS one.
            let gate = lines[..i]
                .iter()
                .rev()
                .find(|l| l.trim_start().starts_with("#[cfg("));
            assert_eq!(
                gate.map(|l| l.trim()),
                Some("#[cfg(target_os = \"macos\")]"),
                "MacBackend at src/main.rs:{} is not gated by cfg(macos)",
                i + 1
            );
        }
    }
}

#[test]
fn unsupported_platform_message_is_shared() {
    for path in ["src/bin/cu-agent.rs", "src/main.rs"] {
        assert!(read(path).contains(MESSAGE), "{path} lacks the message");
    }
}

#[test]
fn docs_state_bin_split() {
    let platform = read("docs/PLATFORM.md");
    assert!(platform.contains("src/bin/cu-agent/app.rs"));
    assert!(!platform.contains("兩個 bin 仍只能在 macOS 編譯"));
    assert!(!read("README.md").contains("兩個 bin 尚不行"));
}
