//! Local/remote host selection, mirroring `horde.sh`'s `pick_host`.
//!
//! The decision logic is separated from the SSH I/O (behind the `Prober`
//! trait) so it can be unit-tested without a network.

use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{bail, Result};

use crate::cli::ForceHost;
use crate::config::Config;

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Local,
    Remote,
}

/// SSH probes, abstracted for testability.
pub trait Prober {
    /// Is the remote reachable within `connect_timeout` seconds?
    fn reachable(&self, remote: &str, connect_timeout: u64) -> bool;
    /// Round-trip latency in milliseconds, or `None` if the probe failed.
    fn latency_ms(&self, remote: &str) -> Option<u128>;
}

struct SshProber;

impl Prober for SshProber {
    fn reachable(&self, remote: &str, connect_timeout: u64) -> bool {
        Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                &format!("ConnectTimeout={connect_timeout}"),
                remote,
                "true",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn latency_ms(&self, remote: &str) -> Option<u128> {
        // Monotonic clock; a warm ControlMaster connection reads near-zero.
        let start = Instant::now();
        let ok = Command::new("ssh")
            .args(["-o", "BatchMode=yes", remote, "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        ok.then(|| start.elapsed().as_millis())
    }
}

pub fn pick_host(config: &Config, force: Option<ForceHost>) -> Result<Decision> {
    decide(
        force,
        config.remote.as_deref(),
        config.latency_ms,
        config.connect_timeout,
        &SshProber,
    )
}

fn decide(
    force: Option<ForceHost>,
    remote: Option<&str>,
    latency_ms: u128,
    connect_timeout: u64,
    prober: &dyn Prober,
) -> Result<Decision> {
    if force == Some(ForceHost::Local) {
        return Ok(Decision::Local);
    }
    let remote = match remote {
        Some(r) => r,
        None => {
            if force == Some(ForceHost::Remote) {
                bail!("--remote given but no remote host is configured");
            }
            return Ok(Decision::Local);
        }
    };
    if !prober.reachable(remote, connect_timeout) {
        if force == Some(ForceHost::Remote) {
            bail!("remote host {remote} is not reachable");
        }
        return Ok(Decision::Local);
    }
    if force == Some(ForceHost::Remote) {
        return Ok(Decision::Remote);
    }
    match prober.latency_ms(remote) {
        Some(ms) if ms <= latency_ms => Ok(Decision::Remote),
        _ => Ok(Decision::Local),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Mock {
        reachable: bool,
        latency: Option<u128>,
        reach_calls: Cell<u32>,
        latency_calls: Cell<u32>,
    }
    impl Mock {
        fn new(reachable: bool, latency: Option<u128>) -> Self {
            Mock {
                reachable,
                latency,
                reach_calls: Cell::new(0),
                latency_calls: Cell::new(0),
            }
        }
    }
    impl Prober for Mock {
        fn reachable(&self, _: &str, _: u64) -> bool {
            self.reach_calls.set(self.reach_calls.get() + 1);
            self.reachable
        }
        fn latency_ms(&self, _: &str) -> Option<u128> {
            self.latency_calls.set(self.latency_calls.get() + 1);
            self.latency
        }
    }

    #[test]
    fn local_force_skips_all_probes() {
        let m = Mock::new(true, Some(1));
        let d = decide(Some(ForceHost::Local), Some("h"), 150, 2, &m).unwrap();
        assert_eq!(d, Decision::Local);
        assert_eq!(m.reach_calls.get(), 0);
    }

    #[test]
    fn no_remote_configured_is_local() {
        let m = Mock::new(true, Some(1));
        assert_eq!(decide(None, None, 150, 2, &m).unwrap(), Decision::Local);
    }

    #[test]
    fn force_remote_without_remote_errors() {
        let m = Mock::new(true, Some(1));
        assert!(decide(Some(ForceHost::Remote), None, 150, 2, &m).is_err());
    }

    #[test]
    fn force_remote_unreachable_errors() {
        let m = Mock::new(false, None);
        assert!(decide(Some(ForceHost::Remote), Some("h"), 150, 2, &m).is_err());
    }

    #[test]
    fn unreachable_falls_back_to_local() {
        let m = Mock::new(false, None);
        assert_eq!(
            decide(None, Some("h"), 150, 2, &m).unwrap(),
            Decision::Local
        );
    }

    #[test]
    fn low_latency_picks_remote() {
        let m = Mock::new(true, Some(50));
        assert_eq!(
            decide(None, Some("h"), 150, 2, &m).unwrap(),
            Decision::Remote
        );
    }

    #[test]
    fn high_latency_picks_local() {
        let m = Mock::new(true, Some(500));
        assert_eq!(
            decide(None, Some("h"), 150, 2, &m).unwrap(),
            Decision::Local
        );
    }

    #[test]
    fn force_remote_reachable_skips_latency() {
        let m = Mock::new(true, Some(9999));
        assert_eq!(
            decide(Some(ForceHost::Remote), Some("h"), 150, 2, &m).unwrap(),
            Decision::Remote
        );
        assert_eq!(m.latency_calls.get(), 0);
    }
}
