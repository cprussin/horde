//! Per-connection relay (`horde-runner serve`), spawned by ssh.
//!
//! Finds or spawns the detached `session` daemon for this project, then
//! shuttles raw protocol bytes between ssh stdio and the daemon's Unix socket.
//! It never decodes frames — both legs speak the same framing.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use crate::config::{check_name, Config};
use crate::runtime;

pub fn serve(config: &Config, project: &str) -> Result<()> {
    // Validate before the name reaches a socket path.
    check_name(&config.projects_dir, project)?;

    let dir = runtime::dir()?;
    let sock = runtime::socket_path(&dir, project);
    let lock = runtime::lock_path(&dir, project);

    let stream = connect_or_spawn(project, &dir, &sock, &lock)?;
    relay(stream)
}

/// Attach to the running daemon, or spawn it.  A per-project flock serialises
/// concurrent `serve` invocations so only one daemon is started.
fn connect_or_spawn(project: &str, dir: &Path, sock: &Path, lock: &Path) -> Result<UnixStream> {
    let lockfile = File::create(lock)?;
    // SAFETY: flock on a valid fd; released when lockfile drops.
    unsafe {
        libc::flock(lockfile.as_raw_fd(), libc::LOCK_EX);
    }

    match UnixStream::connect(sock) {
        Ok(s) => return Ok(s),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            // Absent or stale socket: remove it and spawn a fresh daemon.
            let _ = std::fs::remove_file(sock);
        }
        Err(e) => return Err(e.into()),
    }

    spawn_session(project, dir)?;
    for _ in 0..300 {
        thread::sleep(Duration::from_millis(10));
        if let Ok(s) = UnixStream::connect(sock) {
            return Ok(s);
        }
    }
    bail!("timed out waiting for the session daemon to start");
}

/// Spawn `horde-runner session --project <p>`, fully detached (its own session
/// via setsid) so it outlives this relay and the ssh connection.
fn spawn_session(project: &str, dir: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let log = runtime::log_path(dir, project);
    let logfile = OpenOptions::new().create(true).append(true).open(&log)?;
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o600))?;

    let mut cmd = Command::new(exe);
    cmd.arg("session").arg("--project").arg(project);
    cmd.stdin(Stdio::null());
    cmd.stdout(logfile.try_clone()?);
    cmd.stderr(logfile);
    // SAFETY: setsid in the forked child is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| anyhow!("failed to spawn session daemon: {e}"))?;
    Ok(())
}

/// Copy bytes both ways between ssh stdio and the daemon socket.
fn relay(stream: UnixStream) -> Result<()> {
    let mut from_daemon = stream.try_clone()?;
    let to_daemon = stream;

    let writer = thread::spawn(move || {
        let mut to_daemon = to_daemon;
        let _ = io::copy(&mut io::stdin().lock(), &mut to_daemon);
        let _ = to_daemon.shutdown(Shutdown::Write);
    });

    let mut stdout = io::stdout().lock();
    let _ = io::copy(&mut from_daemon, &mut stdout);
    let _ = stdout.flush();
    // The daemon closed the socket (session exited or we detached); drop the
    // writer with the process.
    drop(writer);
    Ok(())
}
