//! Per-user runtime directory and session socket/lock/log paths.
//!
//! Sockets live under a 0700 directory so only the owning user can reach a
//! live session.  `XDG_RUNTIME_DIR` is preferred but is often unset over a
//! non-login ssh command, so we fall back to `/run/user/<uid>` and finally to
//! an ownership-checked `/tmp/horde-<uid>`.

use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn uid() -> u32 {
    // Safe: getuid is always successful and has no preconditions.
    unsafe { libc::getuid() }
}

/// Return the 0700 `…/horde` directory holding this user's session sockets,
/// creating it if necessary.
pub fn dir() -> io::Result<PathBuf> {
    let base = base_runtime_dir()?;
    let dir = base.join("horde");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

fn base_runtime_dir() -> io::Result<PathBuf> {
    if let Some(d) = env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()) {
        let p = PathBuf::from(d);
        if p.is_dir() {
            return Ok(p);
        }
    }
    let run_user = PathBuf::from(format!("/run/user/{}", uid()));
    if run_user.is_dir() {
        return Ok(run_user);
    }
    // Last resort: /tmp/horde-<uid>, created (or verified) 0700 and owned by us
    // so another user can't pre-create a hostile directory.
    let fallback = PathBuf::from(format!("/tmp/horde-{}", uid()));
    ensure_private_dir(&fallback)?;
    Ok(fallback)
}

fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(meta) => {
            if meta.uid() != uid() || meta.permissions().mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "{} is not a private (0700, owned) directory",
                        path.display()
                    ),
                ));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

pub fn socket_path(dir: &Path, project: &str) -> PathBuf {
    dir.join(format!("{project}.sock"))
}

pub fn lock_path(dir: &Path, project: &str) -> PathBuf {
    dir.join(format!("{project}.lock"))
}

pub fn log_path(dir: &Path, project: &str) -> PathBuf {
    dir.join(format!("{project}.log"))
}
