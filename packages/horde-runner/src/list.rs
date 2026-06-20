//! `horde-runner list` — enumerate live sessions for the client's switcher.
//!
//! Scans the runtime dir for `*.sock`, probes liveness by connecting (a
//! refused/absent socket is stale and gets cleaned up), reads the sibling
//! `<project>.json` metadata, and prints a JSON array of [`SessionMeta`].

use std::fs;
use std::io;
use std::os::unix::net::UnixStream;

use anyhow::Result;
use horde_proto::SessionMeta;

use crate::runtime;

pub fn run() -> Result<()> {
    let dir = runtime::dir()?;
    let mut sessions: Vec<SessionMeta> = Vec::new();

    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        // No runtime dir yet ⇒ no sessions.
        Err(_) => {
            println!("[]");
            return Ok(());
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        match UnixStream::connect(&path) {
            Ok(stream) => {
                // Connect-then-close: the daemon's accept loop treats a probe
                // with no Hello as a no-op.
                drop(stream);
                sessions.push(read_meta(&path));
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                // Stale socket: clean it and its metadata up.
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("json"));
            }
            // Some other error (e.g. permissions): leave it alone, skip it.
            Err(_) => {}
        }
    }

    sessions.sort_by(|a, b| a.project.cmp(&b.project));
    println!("{}", serde_json::to_string(&sessions)?);
    Ok(())
}

/// Read `<project>.json`, falling back to a project name derived from the
/// socket filename for a just-started session that hasn't published metadata.
fn read_meta(sock: &std::path::Path) -> SessionMeta {
    if let Ok(text) = fs::read_to_string(sock.with_extension("json")) {
        if let Ok(meta) = serde_json::from_str::<SessionMeta>(&text) {
            return meta;
        }
    }
    SessionMeta {
        project: sock
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string(),
        ..Default::default()
    }
}
