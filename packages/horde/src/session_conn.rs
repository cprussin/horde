//! A connection to one session, over a transport that speaks the framed
//! protocol on a child process's stdio: locally `horde-runner serve`, or
//! `ssh <host> … horde-runner serve` for a remote.  Both are identical past
//! the spawn — `serve` finds-or-spawns the detached session daemon.

use std::io::BufReader;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use base64::Engine;
use horde_proto::{read_frame, write_frame, ClientFrame, Hello, ServerFrame};

use crate::app::{Msg, SessionId};
use crate::discovery::Host;
use crate::quote::bash_quote;

pub struct SessionConn {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

impl SessionConn {
    /// Spawn the transport, send `Hello`, and start a reader thread forwarding
    /// `ServerFrame`s as `Msg::Frame(id, …)` (and `Msg::Closed(id)` on EOF).
    pub fn connect(
        host: &Host,
        hello: Hello,
        id: SessionId,
        tx: Sender<Msg>,
    ) -> Result<SessionConn> {
        let mut command = transport_command(host, &hello.project);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("failed to start session transport: {e}"))?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        write_frame(&mut stdin, &ClientFrame::Hello(hello))?;
        let stdin = Arc::new(Mutex::new(stdin));

        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(frame)) = read_frame::<_, ServerFrame>(&mut reader) {
                if tx.send(Msg::Frame(id.clone(), frame)).is_err() {
                    return;
                }
            }
            // Transport closed without an Exit: the connection dropped.
            let _ = tx.send(Msg::Closed(id));
        });

        Ok(SessionConn { child, stdin })
    }

    pub fn send_stdin(&self, bytes: Vec<u8>) {
        if let Ok(mut w) = self.stdin.lock() {
            let _ = write_frame(&mut *w, &ClientFrame::Stdin(bytes));
        }
    }

    pub fn send_resize(&self, cols: u16, rows: u16) {
        if let Ok(mut w) = self.stdin.lock() {
            let _ = write_frame(&mut *w, &ClientFrame::Resize { cols, rows });
        }
    }
}

impl Drop for SessionConn {
    fn drop(&mut self) {
        // Detach: killing the transport (ssh / local serve) leaves the detached
        // session daemon — and thus claude — running.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn transport_command(host: &Host, project: &str) -> Command {
    match host {
        Host::Local => {
            let mut c = Command::new("horde-runner");
            c.args(["serve", "--project", project]);
            c
        }
        Host::Remote(h) => {
            let inner = format!("horde-runner serve --project {}", bash_quote(project));
            let remote_cmd = format!("bash -lc {}", bash_quote(&inner));
            let mut c = Command::new("ssh");
            c.arg(h).arg(remote_cmd);
            c
        }
    }
}

/// A human-readable rendering of the transport command, for `--dry-run`.
pub fn dry_run_command(host: &Host, project: &str) -> String {
    match host {
        Host::Local => format!("horde-runner serve --project {}", bash_quote(project)),
        Host::Remote(h) => {
            let inner = format!("horde-runner serve --project {}", bash_quote(project));
            format!("ssh {h} bash -lc {}", bash_quote(&inner))
        }
    }
}

/// Build a `Hello` for a new attach: the launch parameters plus this client's
/// terminal identity (the session can't read it from a non-PTY ssh env).
pub fn make_hello(
    project: &str,
    extras: &[String],
    prompt: &str,
    claude_args: &[String],
    cols: u16,
    rows: u16,
) -> Hello {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    Hello {
        project: project.to_string(),
        extras: extras.to_vec(),
        prompt_b64: base64::engine::general_purpose::STANDARD.encode(prompt.as_bytes()),
        claude_args: claude_args.to_vec(),
        cols,
        rows,
        term: env("TERM"),
        colorterm: env("COLORTERM"),
        lang: env("LANG"),
        lc_all: env("LC_ALL"),
    }
}
