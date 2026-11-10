//! Bounded execution of trusted, read-only helper programs on Unix.
//! A process group improves cleanup but does not contain descendants that detach.
#[cfg(unix)]
use std::os::unix::{io::AsRawFd, process::CommandExt};
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

// These constants are from the supported Darwin and Linux fcntl interfaces.
#[cfg(any(target_os = "macos", target_os = "ios"))]
const NONBLOCK: i32 = 0x4;
#[cfg(any(target_os = "linux", target_os = "android"))]
const NONBLOCK: i32 = 0x800;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
))]
fn nonblocking(fd: i32) -> Result<(), String> {
    let flags = unsafe { fcntl(fd, 3) }; // F_GETFL
    if flags == -1 || unsafe { fcntl(fd, 4, flags | NONBLOCK) } == -1 {
        // F_SETFL
        return Err(format!(
            "cannot set helper pipe nonblocking: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn drain(mut reader: impl Read, limit: usize, deadline: Instant) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    let mut overflow = false;
    loop {
        if Instant::now() >= deadline {
            return Err(
                "helper output did not close before deadline; descendants may remain".into(),
            );
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(
                    Duration::from_millis(5)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        if n == 0 {
            break;
        }
        if n > limit.saturating_sub(out.len()) {
            overflow = true;
        }
        if !overflow {
            out.extend_from_slice(&buf[..n]);
        }
    }
    if overflow {
        Err("helper output exceeded limit".into())
    } else {
        Ok(out)
    }
}

/// Runs a trusted helper with a deadline. Does not prove that detached descendants ended.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
))]
pub fn run(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let deadline = started.checked_add(timeout).ok_or("invalid timeout")?;
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start helper: {e}"))?;
    let stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("stderr unavailable")?;
    // Set these before handing them to reader threads. A descendant can retain
    // either pipe after the leader exits, so a blocking read would leak a thread.
    if let Err(error) =
        nonblocking(stdout.as_raw_fd()).and_then(|_| nonblocking(stderr.as_raw_fd()))
    {
        let _ = child.kill();
        thread::spawn(move || {
            let _ = child.wait();
        });
        return Err(error);
    }
    let (tx, rx) = mpsc::channel();
    for (index, pipe) in [
        (0, Box::new(stdout) as Box<dyn Read + Send>),
        (1, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send((index, drain(pipe, OUTPUT_LIMIT, deadline)));
        });
    }
    drop(tx);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            ),
            Ok(None) => {
                // The leader is still unreaped, so its process group ID cannot have been reused.
                let group_result = unsafe { kill(-(child.id() as i32), 9) };
                let direct_result = child.kill();
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err(format!("helper timed out; group kill result {group_result}; direct kill {direct_result:?}; detached descendants may remain"));
            }
            Err(e) => {
                let _ = child.kill();
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err(format!(
                    "helper wait failed: {e}; descendant cleanup unconfirmed"
                ));
            }
        }
    };
    let mut parts: [Option<Vec<u8>>; 2] = [None, None];
    for _ in 0..2 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let (index, bytes) = rx.recv_timeout(remaining).map_err(|_| {
            "helper output did not close before deadline; descendants may remain".to_string()
        })?;
        parts[index] = Some(bytes?);
    }
    Ok(Output {
        status,
        stdout: parts[0].take().unwrap(),
        stderr: parts[1].take().unwrap(),
    })
}
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
pub fn run(_: &mut Command, _: Duration) -> Result<Output, String> {
    Err("bounded helper execution is unsupported on this platform".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn captures_both_streams() {
        let o = run(
            Command::new("/bin/sh").args(["-c", "printf out; printf err >&2"]),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(o.status.success());
        assert_eq!(o.stdout, b"out");
        assert_eq!(o.stderr, b"err");
    }
    #[test]
    fn deadline_stops_hung_helper() {
        let start = Instant::now();
        let e = run(
            Command::new("/bin/sh").args(["-c", "exec sleep 5"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(e.contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn output_limit_is_enforced_without_deadlock() {
        assert!(drain(
            std::io::repeat(42).take(100),
            10,
            Instant::now() + Duration::from_secs(1)
        )
        .unwrap_err()
        .contains("exceeded"));
    }
    #[test]
    fn reader_stops_at_deadline_even_if_pipe_never_closes() {
        struct NeverReady;
        impl Read for NeverReady {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
        }
        let start = Instant::now();
        let e = drain(NeverReady, 10, start + Duration::from_millis(40)).unwrap_err();
        assert!(e.contains("did not close"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn inherited_pipe_does_not_block_past_deadline() {
        let start = Instant::now();
        let e = run(
            Command::new("/bin/sh").args(["-c", "sleep 1 & exit 0"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(e.contains("did not close"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
