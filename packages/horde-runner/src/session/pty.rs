//! Allocate a PTY and launch bwrap+claude attached to it with the slave as
//! controlling terminal.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::Child;
use std::ptr;

use anyhow::{anyhow, Result};

use crate::bwrap::Sandbox;
use crate::run;

pub struct Pty {
    pub master: File,
    pub child: Child,
}

/// Open a PTY sized `cols`×`rows` and spawn the sandbox attached to its slave.
pub fn spawn(sandbox: &Sandbox, cols: u16, rows: u16) -> Result<Pty> {
    let mut master_raw: libc::c_int = -1;
    let mut slave_raw: libc::c_int = -1;
    let ws = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: valid out-pointers for the two fds and a valid winsize.
    let rc = unsafe {
        libc::openpty(
            &mut master_raw,
            &mut slave_raw,
            ptr::null_mut(),
            ptr::null(),
            &ws,
        )
    };
    if rc != 0 {
        return Err(anyhow!("openpty failed: {}", io::Error::last_os_error()));
    }
    // SAFETY: openpty just handed us these owned fds.
    let master = unsafe { OwnedFd::from_raw_fd(master_raw) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave_raw) };

    let mut cmd = run::command(sandbox);
    let slave_fd = slave.as_raw_fd();
    let master_fd = master.as_raw_fd();
    // SAFETY: the closure runs in the forked child before exec and uses only
    // async-signal-safe syscalls.  Order is load-bearing: become a session
    // leader, take the slave as controlling terminal, wire stdio, then drop the
    // inherited pty fds.
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            for target in 0..3 {
                if libc::dup2(slave_fd, target) == -1 {
                    return Err(io::Error::last_os_error());
                }
            }
            if slave_fd > 2 {
                libc::close(slave_fd);
            }
            libc::close(master_fd);
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .map_err(|e| anyhow!("failed to spawn sandbox: {e}"))?;
    // Close the parent's slave so the master reads EOF once claude exits.
    drop(slave);
    Ok(Pty {
        master: File::from(master),
        child,
    })
}
