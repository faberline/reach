//! One read-only computer-use JSON request per process. Never sends input.
//! The implementation lives in `cu-agent/app.rs` and is macOS-only.
use std::process::ExitCode;

#[cfg(target_os = "macos")]
#[path = "cu-agent/app.rs"]
mod app;

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    app::main()
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    println!(
        "{}",
        serde_json::json!({
            "status": "error",
            "action_allowed": false,
            "reason": "unsupported platform: only macOS is supported",
        })
    );
    ExitCode::FAILURE
}
