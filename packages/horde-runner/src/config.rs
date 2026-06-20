//! Runner configuration read from the environment (the `HORDE_*` variables
//! normally set by the home-manager module), mirroring the head of
//! `horde-run.sh`.

use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Default strict inner-sandbox settings, used when `HORDE_CLAUDE_SETTINGS` is
/// unset (claude's native sandbox nested inside our bubblewrap namespace).
const DEFAULT_SETTINGS: &str =
    r#"{"sandbox":{"enabled":true,"failIfUnavailable":true,"allowUnsandboxedCommands":false}}"#;

pub struct Config {
    pub projects_dir: PathBuf,
    pub state_dir: PathBuf,
    pub sandbox_path: Option<String>,
    pub claude_token_file: Option<String>,
    pub github_token_files: Option<String>,
    pub gh_app_id: Option<String>,
    pub gh_app_key_file: Option<String>,
    pub token_files: Option<String>,
    pub ro_paths: Option<String>,
    pub rw_paths: Option<String>,
    pub allow_nix: Option<String>,
    pub settings: String,
    pub user_name: String,
}

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

        let state_dir = match nonempty("HORDE_STATE_DIR") {
            Some(p) => PathBuf::from(p),
            None => {
                let data =
                    nonempty("XDG_DATA_HOME").unwrap_or_else(|| format!("{home}/.local/share"));
                PathBuf::from(data).join("horde").join("home")
            }
        };

        Config {
            projects_dir,
            state_dir,
            sandbox_path: nonempty("HORDE_SANDBOX_PATH"),
            claude_token_file: nonempty("HORDE_CLAUDE_TOKEN_FILE"),
            github_token_files: nonempty("HORDE_GITHUB_TOKEN_FILES"),
            gh_app_id: nonempty("HORDE_GH_APP_ID"),
            gh_app_key_file: nonempty("HORDE_GH_APP_KEY_FILE"),
            token_files: nonempty("HORDE_TOKEN_FILES"),
            ro_paths: nonempty("HORDE_RO_PATHS"),
            rw_paths: nonempty("HORDE_RW_PATHS"),
            allow_nix: nonempty("HORDE_ALLOW_NIX"),
            settings: nonempty("HORDE_CLAUDE_SETTINGS")
                .unwrap_or_else(|| DEFAULT_SETTINGS.to_string()),
            user_name: nonempty("USER").unwrap_or_else(|| "horde".to_string()),
        }
    }

    pub fn sandbox_home(&self) -> &'static Path {
        Path::new("/home/horde")
    }
}

/// Validate a project name's charset and that it resolves to a directory under
/// `projects_dir` — identical to `horde-run.sh`'s `check_name`.
pub fn check_name(projects_dir: &Path, name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        bail!("invalid project name: {name}");
    }
    if !projects_dir.join(name).is_dir() {
        bail!("no such project: {}/{}", projects_dir.display(), name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_name_matches_bash_rules() {
        let dir = Path::new("/nonexistent");
        for bad in ["", ".x", "a/b", "a b", "a*"] {
            assert_eq!(
                check_name(dir, bad).unwrap_err().to_string(),
                format!("invalid project name: {bad}")
            );
        }
        assert_eq!(
            check_name(dir, "ok.name_1-2").unwrap_err().to_string(),
            "no such project: /nonexistent/ok.name_1-2"
        );
    }
}
