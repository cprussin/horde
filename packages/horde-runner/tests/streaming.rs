//! End-to-end test of the `session` PTY-streaming daemon.
//!
//! Spawns `horde-runner session` with a stub `bwrap` (that execs a stub
//! `claude`), then drives it over the Unix socket using the real protocol:
//! verifies output streaming, input forwarding, resize delivery, reattach +
//! repaint, and exit-code propagation.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use horde_proto::{read_frame, write_frame, ClientFrame, Hello, ServerFrame, SessionMeta};

struct Harness {
    _tmp: PathBuf,
    runtime: PathBuf,
    project: String,
    child: Child,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self._tmp);
    }
}

fn write_exec(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn hello(project: &str, cols: u16, rows: u16) -> ClientFrame {
    ClientFrame::Hello(Hello {
        project: project.to_string(),
        extras: vec![],
        prompt_b64: String::new(),
        claude_args: vec![],
        cols,
        rows,
        term: "xterm-256color".into(),
        colorterm: String::new(),
        lang: "C".into(),
        lc_all: String::new(),
    })
}

impl Harness {
    fn start() -> Harness {
        // Unique per instance so concurrent tests don't share a runtime dir.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("horde-stream-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let bin = tmp.join("bin");
        let projects = tmp.join("projects");
        let project = "p".to_string();
        let runtime = tmp.join("run");
        let state = tmp.join("state");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(projects.join(&project)).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(&state).unwrap();

        // Stub claude: announce readiness, then echo lines; `size` reports the
        // PTY winsize, `quit` exits 7.
        let claude = bin.join("claude-stub");
        write_exec(
            &claude,
            "#!/bin/sh\nprintf 'STUBREADY\\n'\nwhile IFS= read -r line; do\n  case \"$line\" in\n    quit) printf 'BYE\\n'; exit 7 ;;\n    size) stty size ;;\n    *) printf 'ECHO:%s\\n' \"$line\" ;;\n  esac\ndone\n",
        );
        // Stub bwrap: skip args up to `claude`, exec the stub in its place.
        let bwrap = bin.join("bwrap");
        write_exec(
            &bwrap,
            &format!(
                "#!/bin/sh\nwhile [ $# -gt 0 ] && [ \"$1\" != claude ]; do shift; done\n[ \"$1\" = claude ] && shift\nexec {} \"$@\"\n",
                claude.display()
            ),
        );

        let exe = env!("CARGO_BIN_EXE_horde-runner");
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let child = Command::new(exe)
            .args(["session", "--project", &project])
            .env_clear()
            .env("PATH", path)
            .env("HOME", &tmp)
            .env("HORDE_PROJECTS", &projects)
            .env("HORDE_STATE_DIR", &state)
            .env("XDG_RUNTIME_DIR", &runtime)
            .spawn()
            .unwrap();

        Harness {
            _tmp: tmp,
            runtime,
            project,
            child,
        }
    }

    fn socket(&self) -> PathBuf {
        self.runtime
            .join("horde")
            .join(format!("{}.sock", self.project))
    }

    /// Run `horde-runner list` with this harness's runtime dir.
    fn list(&self) -> Vec<SessionMeta> {
        let out = Command::new(env!("CARGO_BIN_EXE_horde-runner"))
            .arg("list")
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &self._tmp)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "list failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Connect, send Hello, and start a reader thread delivering server frames.
    fn attach(&self, cols: u16, rows: u16) -> Conn {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(self.socket()) {
                break s;
            }
            assert!(Instant::now() < deadline, "session socket never appeared");
            thread::sleep(Duration::from_millis(20));
        };
        let mut writer = stream.try_clone().unwrap();
        write_frame(&mut writer, &hello(&self.project, cols, rows)).unwrap();

        let (tx, rx) = mpsc::channel();
        let mut reader = stream;
        thread::spawn(move || {
            while let Ok(Some(frame)) = read_frame::<_, ServerFrame>(&mut reader) {
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        Conn { writer, rx }
    }
}

struct Conn {
    writer: UnixStream,
    rx: Receiver<ServerFrame>,
}

impl Conn {
    fn send(&mut self, frame: ClientFrame) {
        write_frame(&mut self.writer, &frame).unwrap();
    }

    fn type_line(&mut self, line: &str) {
        self.send(ClientFrame::Stdin(format!("{line}\n").into_bytes()));
    }

    /// Read frames until the accumulated output contains `needle`.
    fn await_output(&self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut acc = String::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {needle:?}; got: {acc:?}"
            );
            match self.rx.recv_timeout(remaining) {
                Ok(ServerFrame::Output(b)) => acc.push_str(&String::from_utf8_lossy(&b)),
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for {needle:?}; got: {acc:?}"),
            }
            if acc.contains(needle) {
                return;
            }
        }
    }

    fn await_frame(&self, pred: impl Fn(&ServerFrame) -> bool) -> ServerFrame {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for frame");
            match self.rx.recv_timeout(remaining) {
                Ok(f) if pred(&f) => return f,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for frame"),
            }
        }
    }
}

#[test]
fn streams_input_output_resize_reattach_and_exit() {
    let h = Harness::start();

    // First client: stub output streams through.
    let mut a = h.attach(80, 24);
    a.await_output("STUBREADY");

    // Input is forwarded and echoed back.
    a.type_line("hello");
    a.await_output("ECHO:hello");

    // A resize reaches the PTY (stty inside the stub sees the new size).
    a.send(ClientFrame::Resize {
        cols: 100,
        rows: 40,
    });
    a.type_line("size");
    a.await_output("40 100");

    // Reattach: a second client gets a repaint and a Ready, session still live.
    let b = h.attach(100, 40);
    b.await_frame(|f| matches!(f, ServerFrame::Ready));

    // claude exit propagates to both clients with the right code.
    a.type_line("quit");
    let exit_a = a.await_frame(|f| matches!(f, ServerFrame::Exit(_)));
    assert_eq!(exit_a, ServerFrame::Exit(7));
    let exit_b = b.await_frame(|f| matches!(f, ServerFrame::Exit(_)));
    assert_eq!(exit_b, ServerFrame::Exit(7));
}

#[test]
fn list_reports_live_sessions_and_drops_them_on_exit() {
    let h = Harness::start();
    let mut a = h.attach(80, 24);
    a.await_output("STUBREADY"); // session live ⇒ metadata published

    let sessions = h.list();
    assert_eq!(sessions.len(), 1, "got {sessions:?}");
    assert_eq!(sessions[0].project, "p");
    assert!(sessions[0].pid > 0);

    // After claude exits the daemon removes its socket + metadata.
    a.type_line("quit");
    a.await_frame(|f| matches!(f, ServerFrame::Exit(_)));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if h.list().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "session never dropped from list");
        thread::sleep(Duration::from_millis(50));
    }
}
