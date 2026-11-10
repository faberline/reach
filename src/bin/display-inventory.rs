use serde_json::json;
use std::process::ExitCode;
fn main() -> ExitCode {
    if std::env::args().len() != 1 {
        eprintln!(
            "{}",
            json!({"status":"error","action_allowed":false,"error":"no arguments accepted"})
        );
        return ExitCode::FAILURE;
    }
    match mac_computer_use::display_inventory::observe() {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(reason) => {
            println!(
                "{}",
                json!({"status":"unknown","action_allowed":false,"reason":reason})
            );
            ExitCode::from(4)
        }
    }
}
