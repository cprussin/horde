//! Remote streaming: spawn `ssh <host> horde-runner serve …`, put the local
//! terminal in raw mode, and bridge it to the session over the framed
//! protocol — forwarding keystrokes, rendering PTY output, propagating
//! resizes, and exiting with the session's status.
//!
//! A dropped connection simply detaches: the remote session keeps running and
//! re-running `horde` reattaches.

use std::io::{self, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use horde_proto::{read_frame, write_frame, ClientFrame, Hello, ServerFrame};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

use crate::quote::bash_quote;

pub struct Remote<'a> {
    pub host: &'a str,
    pub project: &'a str,
    pub extras: &'a [String],
    pub prompt: &'a str,
    pub claude_args: &'a [String],
}

/// Build the command run on the remote host: a login shell (so the runner's
/// HORDE_* environment is loaded) running `horde-runner serve`.
pub fn remote_command(project: &str) -> String {
    let inner = format!("horde-runner serve --project {}", bash_quote(project));
    format!("bash -lc {}", bash_quote(&inner))
}

pub fn attach(remote: &Remote) -> Result<()> {
    let mut child = Command::new("ssh")
        .arg(remote.host)
        .arg(remote_command(remote.project))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow!("failed to start ssh: {e}"))?;

    let ssh_in = Arc::new(Mutex::new(child.stdin.take().expect("piped stdin")));
    let ssh_out = child.stdout.take().expect("piped stdout");

    enable_raw_mode().map_err(|e| anyhow!("failed to enter raw mode: {e}"))?;
    // From here on, restore the terminal on every exit path.
    let result = stream(remote, &ssh_in, ssh_out, &mut child);
    let _ = disable_raw_mode();

    match result {
        Ok(code) => {
            let _ = child.wait();
            std::process::exit(code);
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(e)
        }
    }
}

fn stream(
    remote: &Remote,
    ssh_in: &Arc<Mutex<std::process::ChildStdin>>,
    ssh_out: std::process::ChildStdout,
    _child: &mut Child,
) -> Result<i32> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let hello = Hello {
        project: remote.project.to_string(),
        extras: remote.extras.to_vec(),
        prompt_b64: base64::engine::general_purpose::STANDARD.encode(remote.prompt.as_bytes()),
        claude_args: remote.claude_args.to_vec(),
        cols,
        rows,
        term: env("TERM"),
        colorterm: env("COLORTERM"),
        lang: env("LANG"),
        lc_all: env("LC_ALL"),
    };
    {
        let mut w = ssh_in.lock().unwrap();
        write_frame(&mut *w, &ClientFrame::Hello(hello))?;
    }

    spawn_stdin_forwarder(Arc::clone(ssh_in));
    spawn_resize_forwarder(Arc::clone(ssh_in))?;

    // Main loop: render server output until the session exits or ssh closes.
    let mut reader = BufReader::new(ssh_out);
    let mut stdout = io::stdout();
    loop {
        match read_frame::<_, ServerFrame>(&mut reader)? {
            Some(ServerFrame::Output(bytes)) => {
                stdout.write_all(&bytes)?;
                stdout.flush()?;
            }
            Some(ServerFrame::Ready) => {}
            Some(ServerFrame::Exit(code)) => return Ok(code),
            Some(ServerFrame::Error(msg)) => bail!("{msg}"),
            // ssh closed without an Exit frame: the connection dropped, which
            // detaches; the remote session keeps running.
            None => return Ok(0),
        }
    }
}

/// Forward raw terminal input to the session as Stdin frames.
fn spawn_stdin_forwarder(ssh_in: Arc<Mutex<std::process::ChildStdin>>) {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut w = ssh_in.lock().unwrap();
                    if write_frame(&mut *w, &ClientFrame::Stdin(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// Forward SIGWINCH as Resize frames.
fn spawn_resize_forwarder(ssh_in: Arc<Mutex<std::process::ChildStdin>>) -> Result<()> {
    let mut signals = Signals::new([SIGWINCH]).map_err(|e| anyhow!("signal setup failed: {e}"))?;
    thread::spawn(move || {
        for _ in signals.forever() {
            if let Ok((cols, rows)) = crossterm::terminal::size() {
                let mut w = ssh_in.lock().unwrap();
                let _ = write_frame(&mut *w, &ClientFrame::Resize { cols, rows });
            }
        }
    });
    Ok(())
}
