//! Cooperative, fail-fast exclusion for this user's framework capture helpers.
use std::{
    fs::{self, File, OpenOptions},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawFd,
    },
    path::Path,
};

unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn geteuid() -> u32;
}
const O_NOFOLLOW: i32 = 0x100;
const LOCK_EX_NB: i32 = 0x02 | 0x04;

/// The file must remain in place. flock releases the lock when this handle closes.
#[derive(Debug)]
pub struct CaptureLock {
    _file: File,
}

fn acquire_at(path: &Path) -> Result<CaptureLock, String> {
    let uid = unsafe { geteuid() };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .map_err(|e| format!("capture lock unavailable: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("capture lock inspect failed: {e}"))?;
    if !meta.is_file() || meta.uid() != uid || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
        return Err("capture lock is not a private regular file owned by this user".into());
    }
    if unsafe { flock(file.as_raw_fd(), LOCK_EX_NB) } != 0 {
        return Err(format!(
            "capture busy or lock failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let named =
        fs::symlink_metadata(path).map_err(|e| format!("capture lock path unavailable: {e}"))?;
    if !named.is_file() || named.dev() != meta.dev() || named.ino() != meta.ino() {
        return Err("capture lock path changed".into());
    }
    Ok(CaptureLock { _file: file })
}

pub fn acquire() -> Result<CaptureLock, String> {
    acquire_at(Path::new(&format!(
        "/private/tmp/mac-computer-use-capture-{}.lock",
        unsafe { geteuid() }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn exclusive_and_released_on_drop() {
        let dir = std::env::temp_dir().join(format!("cu-lock-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("lock");
        let held = acquire_at(&path).unwrap();
        assert!(acquire_at(&path).unwrap_err().contains("capture busy"));
        drop(held);
        assert!(acquire_at(&path).is_ok());
        fs::remove_file(&path).unwrap();
        symlink("/dev/null", &path).unwrap();
        assert!(acquire_at(&path).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
