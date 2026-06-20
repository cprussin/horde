//! Detached PTY session daemon (`horde-runner session`).
//!
//! Owns the claude PTY for the lifetime of the session, independent of any
//! client connection.  Clients attach over a Unix socket; the daemon
//! broadcasts PTY output to all attached clients, forwards their input to the
//! PTY, repaints the current screen on attach (so a reattach is crisp), and
//! exits when claude exits.

mod pty;

use std::fs::File;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use horde_proto::{read_frame, write_frame, ClientFrame, Hello, ServerFrame};

use crate::bwrap::TermEnv;
use crate::cli::RunArgs;
use crate::config::{check_name, Config};
use crate::{run, runtime};

struct Client {
    id: u64,
    tx: Sender<ServerFrame>,
}

struct State {
    master_write: Mutex<File>,
    master_fd: RawFd,
    screen: Mutex<vt100::Parser>,
    clients: Mutex<Vec<Client>>,
    next_id: AtomicU64,
}

pub fn run(config: &Config, project: &str) -> Result<()> {
    let dir = runtime::dir()?;
    let sock = runtime::socket_path(&dir, project);

    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        // Another daemon won the race; nothing to do.
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Ok(()),
        Err(e) => return Err(anyhow!("bind {}: {e}", sock.display())),
    };
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;

    let result = serve_loop(config, project, &sock, listener);
    let _ = std::fs::remove_file(&sock);
    result
}

fn serve_loop(
    config: &Config,
    project: &str,
    sock: &std::path::Path,
    listener: UnixListener,
) -> Result<()> {
    // The first client triggers the sandbox build (its Hello carries the
    // launch parameters and terminal size).
    let (mut first, _) = listener.accept().map_err(|e| anyhow!("accept: {e}"))?;
    let hello = read_hello(&mut first)?;
    if hello.project != project {
        bail!("session project mismatch: {} != {project}", hello.project);
    }
    check_name(&config.projects_dir, project)?;
    for name in &hello.extras {
        check_name(&config.projects_dir, name)?;
    }

    let sandbox = run::prepare(config, &run_args_from(&hello), &term_from(&hello))?;
    let pty = pty::spawn(&sandbox, hello.cols, hello.rows)?;
    let master_read = pty.master.try_clone()?;
    let master_fd = pty.master.as_raw_fd();
    let state = Arc::new(State {
        master_write: Mutex::new(pty.master),
        master_fd,
        screen: Mutex::new(vt100::Parser::new(hello.rows.max(1), hello.cols.max(1), 0)),
        clients: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(0),
    });

    // PTY output → screen model + broadcast.
    {
        let state = Arc::clone(&state);
        thread::spawn(move || pty_reader(&state, master_read));
    }
    // The first client (Hello already consumed).
    {
        let state = Arc::clone(&state);
        thread::spawn(move || handle_client(&state, first, Some(hello)));
    }
    // claude exit → notify clients, clean up, exit the process.
    {
        let state = Arc::clone(&state);
        let sock = sock.to_path_buf();
        let mut child = pty.child;
        thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
            broadcast(&state, ServerFrame::Exit(code));
            // Give the per-client writers a moment to flush the Exit frame.
            thread::sleep(Duration::from_millis(150));
            let _ = std::fs::remove_file(&sock);
            std::process::exit(0);
        });
    }

    // Reattaching clients.
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let state = Arc::clone(&state);
                thread::spawn(move || handle_client(&state, s, None));
            }
            Err(_) => break,
        }
    }
    Ok(())
}

fn pty_reader(state: &State, mut master: File) {
    let mut buf = [0u8; 8192];
    loop {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let data = buf[..n].to_vec();
                // Hold the screen lock across process + broadcast so an
                // attaching client's repaint can't interleave with live output.
                let mut screen = state.screen.lock().unwrap();
                screen.process(&data);
                let clients = state.clients.lock().unwrap();
                for c in clients.iter() {
                    let _ = c.tx.send(ServerFrame::Output(data.clone()));
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

fn handle_client(state: &State, mut stream: UnixStream, pre_hello: Option<Hello>) {
    let hello = match pre_hello {
        Some(h) => h,
        None => match read_hello(&mut stream) {
            Ok(h) => h,
            Err(_) => return,
        },
    };
    // Apply this client's terminal size.
    resize(state, hello.cols, hello.rows);

    let (tx, rx) = channel::<ServerFrame>();
    let id = state.next_id.fetch_add(1, Ordering::Relaxed);

    let mut wstream = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let writer = thread::spawn(move || {
        for frame in rx {
            let is_exit = matches!(frame, ServerFrame::Exit(_));
            if write_frame(&mut wstream, &frame).is_err() {
                break;
            }
            if is_exit {
                let _ = wstream.shutdown(Shutdown::Both);
                break;
            }
        }
    });

    // Register and repaint under the screen lock so the repaint precedes any
    // live output forwarded to this client.
    {
        let screen = state.screen.lock().unwrap();
        let _ = tx.send(ServerFrame::Output(repaint(&screen)));
        let _ = tx.send(ServerFrame::Ready);
        state.clients.lock().unwrap().push(Client { id, tx });
    }

    // Forward client input until it disconnects.
    loop {
        match read_frame::<_, ClientFrame>(&mut stream) {
            Ok(Some(ClientFrame::Stdin(bytes))) => {
                let mut mw = state.master_write.lock().unwrap();
                if mw.write_all(&bytes).and_then(|()| mw.flush()).is_err() {
                    break;
                }
            }
            Ok(Some(ClientFrame::Resize { cols, rows })) => resize(state, cols, rows),
            Ok(Some(ClientFrame::Hello(h))) => resize(state, h.cols, h.rows),
            Ok(None) | Err(_) => break,
        }
    }

    state.clients.lock().unwrap().retain(|c| c.id != id);
    let _ = writer.join();
}

fn resize(state: &State, cols: u16, rows: u16) {
    let ws = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // Setting the master's size delivers SIGWINCH to claude automatically.
    unsafe {
        libc::ioctl(state.master_fd, libc::TIOCSWINSZ, &ws);
    }
    state
        .screen
        .lock()
        .unwrap()
        .set_size(rows.max(1), cols.max(1));
}

fn broadcast(state: &State, frame: ServerFrame) {
    let clients = state.clients.lock().unwrap();
    for c in clients.iter() {
        let _ = c.tx.send(frame.clone());
    }
}

/// Bytes that repaint the current screen on a freshly-attached terminal:
/// clear, then the vt100 model's formatted contents.
fn repaint(screen: &vt100::Parser) -> Vec<u8> {
    let mut out = b"\x1b[2J\x1b[H".to_vec();
    out.extend_from_slice(&screen.screen().contents_formatted());
    out
}

fn read_hello(stream: &mut UnixStream) -> Result<Hello> {
    match read_frame::<_, ClientFrame>(stream)? {
        Some(ClientFrame::Hello(h)) => Ok(h),
        _ => bail!("expected a Hello frame"),
    }
}

fn run_args_from(hello: &Hello) -> RunArgs {
    RunArgs {
        project: hello.project.clone(),
        extra_projects: hello.extras.clone(),
        prompt_b64: hello.prompt_b64.clone(),
        dry_run: false,
        claude_args: hello.claude_args.clone(),
    }
}

fn term_from(hello: &Hello) -> TermEnv {
    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
    TermEnv {
        term: opt(&hello.term),
        colorterm: opt(&hello.colorterm),
        lang: opt(&hello.lang),
        lc_all: opt(&hello.lc_all),
    }
}
