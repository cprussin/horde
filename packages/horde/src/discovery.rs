//! Discover running sessions across the local machine and the configured
//! remotes by running `horde-runner list` on each.

use std::process::Command;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use horde_proto::SessionMeta;

use crate::app::Msg;
use crate::quote::bash_quote;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    Local,
    Remote(String),
}

impl Host {
    pub fn label(&self) -> &str {
        match self {
            Host::Local => "local",
            Host::Remote(h) => h,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub host: Host,
    pub meta: SessionMeta,
}

/// Run `horde-runner list` locally.
pub fn list_local() -> Vec<SessionMeta> {
    run_list(Command::new("horde-runner").arg("list"))
}

/// Run `horde-runner list` on a remote host over ssh (login shell so HORDE_*
/// loads); a down/unreachable host contributes nothing.
pub fn list_remote(host: &str, connect_timeout: u64) -> Vec<SessionMeta> {
    let remote_cmd = format!("bash -lc {}", bash_quote("horde-runner list"));
    run_list(Command::new("ssh").args([
        "-o",
        "BatchMode=yes",
        "-o",
        &format!("ConnectTimeout={connect_timeout}"),
        host,
        &remote_cmd,
    ]))
}

fn run_list(cmd: &mut Command) -> Vec<SessionMeta> {
    match cmd.output() {
        Ok(out) if out.status.success() => serde_json::from_slice(&out.stdout).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Aggregate local + per-remote sessions (remotes probed in parallel).
pub fn discover(remotes: &[String], connect_timeout: u64) -> Vec<SessionInfo> {
    let mut out: Vec<SessionInfo> = list_local()
        .into_iter()
        .map(|meta| SessionInfo {
            host: Host::Local,
            meta,
        })
        .collect();

    let handles: Vec<_> = remotes
        .iter()
        .map(|host| {
            let host = host.clone();
            thread::spawn(move || {
                list_remote(&host, connect_timeout)
                    .into_iter()
                    .map(|meta| SessionInfo {
                        host: Host::Remote(host.clone()),
                        meta,
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    for h in handles {
        if let Ok(v) = h.join() {
            out.extend(v);
        }
    }
    out
}

/// Spawn a background worker that re-discovers every `interval` and pushes a
/// `Msg::Discovery` snapshot.
pub fn spawn_worker(
    remotes: Vec<String>,
    connect_timeout: u64,
    interval: Duration,
    tx: Sender<Msg>,
) {
    thread::spawn(move || loop {
        let sessions = discover(&remotes, connect_timeout);
        if tx.send(Msg::Discovery(sessions)).is_err() {
            break;
        }
        thread::sleep(interval);
    });
}
