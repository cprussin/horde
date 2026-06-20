//! Non-interactive one-shot streaming: connect to a single session, relay its
//! PTY output to stdout and our stdin to it, and exit with its status.  Used
//! when stdout/stdin aren't both TTYs (e.g. `horde --project api "…" -- -p`),
//! where the full multiplexer UI can't run.

use std::io::{self, BufReader, Read, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, bail, Result};
use horde_proto::{read_frame, write_frame, ClientFrame, Hello, ServerFrame};

use crate::discovery::Host;
use crate::session_conn::transport_command;

/// Stream one session to/from this process's stdio; returns its exit code.
pub fn stream_oneshot(host: &Host, hello: Hello) -> Result<i32> {
    let mut child = transport_command(host, &hello.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow!("failed to start session transport: {e}"))?;

    let mut to_session = child.stdin.take().expect("piped stdin");
    let from_session = child.stdout.take().expect("piped stdout");
    write_frame(&mut to_session, &ClientFrame::Hello(hello))?;
    let to_session = Arc::new(Mutex::new(to_session));

    // Forward our stdin to the session.
    {
        let to_session = Arc::clone(&to_session);
        thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut w = to_session.lock().unwrap();
                        if write_frame(&mut *w, &ClientFrame::Stdin(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    let mut reader = BufReader::new(from_session);
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
            None => return Ok(0), // transport closed (detached)
        }
    }
}
