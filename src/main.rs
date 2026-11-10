use mac_computer_use::{capabilities, unsupported_action};
#[cfg(target_os = "macos")]
use mac_computer_use::{observe_with_evidence, Backend, MacBackend, Target};
use serde_json::json;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::{env, process::ExitCode};

#[cfg(not(target_os = "macos"))]
fn run(args: &[String]) -> Result<serde_json::Value, String> {
    match args {
        [cmd] if cmd == "capabilities" => Ok(capabilities()),
        [cmd] if cmd == "action" => Ok(unsupported_action()),
        _ => Err("unsupported platform: only macOS is supported".into()),
    }
}

#[cfg(target_os = "macos")]
fn run(args: &[String]) -> Result<serde_json::Value, String> {
    if matches!(args, [cmd] if cmd == "capabilities") {
        return Ok(capabilities());
    }
    let backend = MacBackend::adjacent_to_executable()?;
    match args {
        [cmd, flag, pid] if cmd == "windows" && flag == "--pid" => {
            let pid = pid.parse::<i32>().map_err(|_| "invalid PID")?;
            let windows = backend.windows(Some(pid))?.into_iter().map(|w| json!({
                "pid":w.target.pid,"window_id":w.target.window_id,
                "bounds_points":{"x":w.bounds.x,"y":w.bounds.y,"width":w.bounds.width,"height":w.bounds.height},
                "layer":w.layer,"title":w.title.clone(),"surface_class":w.surface_class(),"action_allowed":false
            })).collect::<Vec<_>>();
            Ok(json!({"status":"observed", "coordinate_system":"quartz_screen_points_top_left",
                "action_allowed":false, "note":"normal layer is only a candidate; surface type, visibility and focus are not proven", "windows":windows}))
        }
        [cmd, flag, pid] if cmd == "accessibility" && flag == "--pid" => {
            let pid = pid.parse::<i32>().map_err(|_| "invalid PID")?;
            backend.accessibility(pid)
        }
        [cmd, pflag, pid, wflag, window] if cmd == "observe-binding" && pflag == "--pid" && wflag == "--window" => {
            let target = Target::new(pid.parse().map_err(|_| "invalid PID")?, window.parse().map_err(|_| "invalid window ID")?)?;
            Ok(mac_computer_use::live_ax_cg::observe(&backend, target))
        }
        [cmd, pflag, pid, wflag, window, oflag, output]
            if cmd == "capture" && pflag == "--pid" && wflag == "--window" && oflag == "--output" => {
            let target = Target::new(pid.parse().map_err(|_| "invalid PID")?, window.parse().map_err(|_| "invalid window ID")?)?;
            let image = Path::new(output);
            observe_with_evidence(&backend, target, image)
        }
        [cmd] if cmd == "action" => Ok(unsupported_action()),
        _ => Err("usage: mac-computer-use capabilities | windows --pid PID | accessibility --pid PID | observe-binding --pid PID --window ID | capture --pid PID --window ID --output /absolute/new.png | action".into()),
    }
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match run(&args) {
        Ok(v) => {
            if v["status"] == "error" {
                eprintln!("{}", v);
                ExitCode::FAILURE
            } else if v["status"] == "unknown" {
                println!("{}", v);
                ExitCode::from(4)
            } else if v["status"] == "unsupported" {
                println!("{}", v);
                ExitCode::from(3)
            } else {
                println!("{}", v);
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!(
                "{}",
                json!({"status":"error", "action_allowed":false, "error":e})
            );
            ExitCode::FAILURE
        }
    }
}
