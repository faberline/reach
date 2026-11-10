//! MCP stdio server. Every tool call runs one `cu-agent` process. Never sends input.
use mac_computer_use::mcp_server;
use std::{env, io, path::PathBuf, process::ExitCode};

const USAGE: &str =
    "usage: cu-mcp --cu-agent <absolute path of cu-agent> [--helper <field>=<absolute path>]...";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (program, helpers) = match args.as_slice() {
        [flag, path, rest @ ..] if flag == "--cu-agent" && PathBuf::from(path).is_absolute() => {
            match mcp_server::parse_helpers(rest) {
                Ok(helpers) => (PathBuf::from(path), helpers),
                Err(reason) => {
                    eprintln!("cu-mcp: {reason}");
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let run =
        |request: &[u8]| mcp_server::run_cu_agent(&program, request, mcp_server::CALL_TIMEOUT);
    match mcp_server::serve(io::stdin().lock(), io::stdout().lock(), run, helpers) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cu-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
