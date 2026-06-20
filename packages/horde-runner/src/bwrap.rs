//! Assemble the bubblewrap argument vector, the launched process's
//! environment, and the claude command — a faithful port of the sandbox
//! assembly in `horde-run.sh` (lines 246-434).

use std::env;
use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Result};
use base64::Engine;

use crate::cli::RunArgs;
use crate::config::Config;
use crate::ghwrapper;
use crate::gitconfig;
use crate::secrets::Secrets;
use crate::util::which;

/// Terminal identity for the sandbox.  These can't be read from a non-PTY ssh
/// environment, so the remote path supplies them from the client's `Hello`;
/// the local path reads them from its own environment.
#[derive(Debug, Default, Clone)]
pub struct TermEnv {
    pub term: Option<String>,
    pub colorterm: Option<String>,
    pub lang: Option<String>,
    pub lc_all: Option<String>,
}

impl TermEnv {
    pub fn from_env() -> Self {
        let get = |k: &str| env::var(k).ok().filter(|s| !s.is_empty());
        TermEnv {
            term: get("TERM"),
            colorterm: get("COLORTERM"),
            lang: get("LANG"),
            lc_all: get("LC_ALL"),
        }
    }

    fn pairs(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        for (k, val) in [
            ("TERM", &self.term),
            ("COLORTERM", &self.colorterm),
            ("LANG", &self.lang),
            ("LC_ALL", &self.lc_all),
        ] {
            if let Some(val) = val {
                v.push((k.to_string(), val.clone()));
            }
        }
        v
    }
}

pub struct Sandbox {
    pub bwrap_args: Vec<String>,
    /// Environment for the bwrap process (it is inherited into the sandbox;
    /// secrets live here, never on the argv).
    pub child_env: Vec<(String, String)>,
    pub command: Vec<String>,
}

/// Build the sandbox.  This has the same side effects as the bash version
/// (creates the state dir, writes the gitconfig and `gh` wrapper) so that
/// `--dry-run` reflects the real setup.
pub fn assemble(
    config: &Config,
    run: &RunArgs,
    secrets: &Secrets,
    term: &TermEnv,
) -> Result<Sandbox> {
    let projects_dir = &config.projects_dir;
    let project_path = projects_dir.join(&run.project);
    let sandbox_home = config.sandbox_home();
    let state_dir = &config.state_dir;

    fs::create_dir_all(state_dir)?;

    let host_path = env::var("PATH").unwrap_or_default();
    let mut inner_path = match &config.sandbox_path {
        Some(p) => format!("{p}:{host_path}"),
        None => host_path.clone(),
    };

    // --- github credential config + gh wrapper ---
    let github_configured =
        secrets.have_gh_default || !secrets.gh_owners.is_empty() || secrets.have_gh_app;
    if github_configured {
        let gh_app_helper = if secrets.have_gh_app {
            Some(
                which("horde-gh-app-credential")
                    .ok_or_else(|| anyhow!("horde-gh-app-credential not found on PATH"))?
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        };
        gitconfig::write(state_dir, sandbox_home, secrets, gh_app_helper.as_deref())?;
    }

    let needs_gh_wrapper = !secrets.gh_owners.is_empty() || secrets.have_gh_app;
    if needs_gh_wrapper {
        let real_bash = which("bash").ok_or_else(|| anyhow!("bash not found on PATH"))?;
        let wrapper_dir = sandbox_home.join("bin");
        let wrapper_dir = wrapper_dir.to_string_lossy();
        ghwrapper::write(state_dir, &real_bash.to_string_lossy(), &wrapper_dir)?;
        inner_path = format!("{wrapper_dir}:{inner_path}");
    }

    // --- bwrap args ---
    let mut a: Vec<String> = Vec::new();
    let push = |a: &mut Vec<String>, items: &[&str]| a.extend(items.iter().map(|s| s.to_string()));

    push(
        &mut a,
        &["--die-with-parent", "--unshare-all", "--share-net"],
    );
    push(
        &mut a,
        &["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"],
    );
    push(&mut a, &["--ro-bind", "/nix/store", "/nix/store"]);
    a.extend([
        "--bind".into(),
        state_dir.to_string_lossy().into_owned(),
        sandbox_home.to_string_lossy().into_owned(),
    ]);
    let project_path_s = project_path.to_string_lossy().into_owned();
    a.extend([
        "--bind".into(),
        project_path_s.clone(),
        project_path_s.clone(),
    ]);
    a.extend(["--chdir".into(), project_path_s]);
    a.extend([
        "--setenv".into(),
        "HOME".into(),
        sandbox_home.to_string_lossy().into_owned(),
    ]);
    a.extend(["--setenv".into(), "USER".into(), config.user_name.clone()]);
    push(&mut a, &["--setenv", "XDG_RUNTIME_DIR", "/tmp"]);
    a.extend(["--setenv".into(), "PATH".into(), inner_path]);

    for name in &run.extra_projects {
        let p = projects_dir.join(name).to_string_lossy().into_owned();
        a.extend(["--bind".into(), p.clone(), p]);
    }

    // Minimal /etc plumbing (bind only what exists on this host).
    for path in [
        "/etc/resolv.conf",
        "/etc/ssl",
        "/etc/static",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/passwd",
        "/etc/group",
        "/etc/localtime",
        "/etc/machine-id",
        "/bin/sh",
        "/usr/bin/env",
    ] {
        if Path::new(path).exists() {
            a.extend(["--ro-bind".into(), path.into(), path.into()]);
        }
    }

    if Path::new("/etc/ssl/certs/ca-certificates.crt").exists() {
        push(
            &mut a,
            &[
                "--setenv",
                "SSL_CERT_FILE",
                "/etc/ssl/certs/ca-certificates.crt",
            ],
        );
        push(
            &mut a,
            &[
                "--setenv",
                "NIX_SSL_CERT_FILE",
                "/etc/ssl/certs/ca-certificates.crt",
            ],
        );
    }

    if matches!(
        config.allow_nix.as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) {
        if Path::new("/nix/var/nix/daemon-socket").exists() {
            push(
                &mut a,
                &[
                    "--bind",
                    "/nix/var/nix/daemon-socket",
                    "/nix/var/nix/daemon-socket",
                ],
            );
        }
        if Path::new("/etc/nix").exists() {
            push(&mut a, &["--ro-bind", "/etc/nix", "/etc/nix"]);
        }
        push(&mut a, &["--setenv", "NIX_REMOTE", "daemon"]);
    }

    for (json, ro) in [(&config.ro_paths, true), (&config.rw_paths, false)] {
        if let Some(json) = json {
            for path in parse_paths(json)? {
                if !Path::new(&path).exists() {
                    let kind = if ro { "read-only" } else { "read-write" };
                    bail!("exposed {kind} path does not exist: {path}");
                }
                let flag = if ro { "--ro-bind" } else { "--bind" };
                a.extend([flag.into(), path.clone(), path]);
            }
        }
    }

    // --- child env: keep-list + secrets (PATH kept so bwrap is found) ---
    let mut child_env = vec![("PATH".to_string(), host_path)];
    child_env.extend(term.pairs());
    child_env.extend(secrets.vars.iter().cloned());

    // --- claude command ---
    let mut command = vec![
        "claude".to_string(),
        "--dangerously-skip-permissions".to_string(),
        "--settings".to_string(),
        config.settings.clone(),
    ];
    for name in &run.extra_projects {
        command.push("--add-dir".into());
        command.push(projects_dir.join(name).to_string_lossy().into_owned());
    }
    command.extend(run.claude_args.iter().cloned());
    if !run.prompt_b64.is_empty() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(run.prompt_b64.as_bytes())
            .map_err(|_| anyhow!("could not decode prompt"))?;
        let prompt = String::from_utf8(bytes).map_err(|_| anyhow!("could not decode prompt"))?;
        command.push(prompt);
    }

    Ok(Sandbox {
        bwrap_args: a,
        child_env,
        command,
    })
}

fn parse_paths(json: &str) -> Result<Vec<String>> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| anyhow!("invalid path JSON: {e}"))?;
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow!("expected a JSON array of paths"))?;
    arr.iter()
        .filter_map(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| Ok(s.to_string()))
        .collect()
}
