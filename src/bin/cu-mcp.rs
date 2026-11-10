//! MCP stdio server. Every tool call runs one `cu-agent` process. Never sends input.
use mac_computer_use::mcp_server;
use std::{env, io, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let program = match args.as_slice() {
        [flag, path] if flag == "--cu-agent" && PathBuf::from(path).is_absolute() => {
            PathBuf::from(path)
        }
        _ => {
            eprintln!("usage: cu-mcp --cu-agent <absolute path of cu-agent>");
            return ExitCode::from(2);
        }
    };
    match mcp_server::serve(io::stdin().lock(), io::stdout().lock(), |request| {
        mcp_server::run_cu_agent(&program, request, mcp_server::CALL_TIMEOUT)
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cu-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
