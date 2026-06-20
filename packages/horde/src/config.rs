//! Configuration resolved from the environment, mirroring `horde.sh`'s
//! defaults.  CLI flags (`--host`) are applied on top by `main`.

use std::env;
use std::path::PathBuf;

pub struct Config {
    pub projects_dir: PathBuf,
    pub remote: Option<String>,
    /// Extra hosts the session switcher discovers sessions on (HORDE_REMOTES).
    pub remotes: Vec<String>,
    pub latency_ms: u128,
    pub connect_timeout: u64,
    pub router_model: String,
    pub claude_token_file: Option<String>,
    pub history_file: PathBuf,
}

/// Treat an empty environment variable as unset, like bash's `${VAR:-default}`.
fn nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.is_empty())
}

impl Config {
    pub fn from_env() -> Self {
        let home = env::var("HOME").unwrap_or_default();

        let projects_dir = match nonempty("HORDE_PROJECTS") {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(&home).join("Projects"),
        };

        let latency_ms = nonempty("HORDE_LATENCY_MS")
            .and_then(|s| s.parse().ok())
            .unwrap_or(150);
        let connect_timeout = nonempty("HORDE_CONNECT_TIMEOUT")
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        let router_model =
            nonempty("HORDE_ROUTER_MODEL").unwrap_or_else(|| "claude-haiku-4-5".to_string());

        let history_file = match nonempty("HORDE_HISTORY_FILE") {
            Some(p) => PathBuf::from(p),
            None => {
                let state =
                    nonempty("XDG_STATE_HOME").unwrap_or_else(|| format!("{home}/.local/state"));
                PathBuf::from(state).join("horde").join("prompt-history")
            }
        };

        let remotes = nonempty("HORDE_REMOTES")
            .map(|s| {
                s.split([',', ' '])
                    .filter(|x| !x.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        Config {
            projects_dir,
            remote: nonempty("HORDE_REMOTE"),
            remotes,
            latency_ms,
            connect_timeout,
            router_model,
            claude_token_file: nonempty("HORDE_CLAUDE_TOKEN_FILE"),
            history_file,
        }
    }

    /// Hosts the switcher discovers sessions on: the primary remote plus
    /// `remotes`, de-duplicated (local is always included separately).
    pub fn discovery_remotes(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for host in self.remote.iter().chain(self.remotes.iter()) {
            if !out.contains(host) {
                out.push(host.clone());
            }
        }
        out
    }
}
