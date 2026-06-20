//! Hand off to the runner: locally by exec'ing `horde-runner run` in this
//! terminal, or remotely by streaming a `horde-runner serve` session over ssh.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{bail, Result};
use base64::Engine;

use crate::attach::{self, Remote};
use crate::config::Config;
use crate::host::Decision;

/// Assemble the `horde-runner run` argument vector for a local launch.
pub fn run_args(
    primary: &str,
    extras: &[String],
    prompt: &str,
    claude_args: &[String],
) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--project".to_string(),
        primary.to_string(),
    ];
    for name in extras {
        args.push("--add".to_string());
        args.push(name.clone());
    }
    if !prompt.is_empty() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(prompt.as_bytes());
        args.push("--prompt-b64".to_string());
        args.push(b64);
    }
    if !claude_args.is_empty() {
        args.push("--".to_string());
        args.extend(claude_args.iter().cloned());
    }
    args
}

/// Either print the resolved command (`--dry-run`) or hand off.
pub fn dispatch(
    config: &Config,
    selected: &[String],
    prompt: &str,
    claude_args: &[String],
    decision: Decision,
    dry_run: bool,
) -> Result<()> {
    let primary = &selected[0];
    let extras = &selected[1..];

    match decision {
        Decision::Local => {
            let args = run_args(primary, extras, prompt, claude_args);
            if dry_run {
                println!("projects: {}", selected.join(" "));
                println!("host:     local");
                println!("command:  horde-runner {}", args.join(" "));
                return Ok(());
            }
            // Replace this process with the local runner in the same terminal.
            let err = Command::new("horde-runner").args(&args).exec();
            bail!("failed to exec horde-runner: {err}");
        }
        Decision::Remote => {
            let host = config
                .remote
                .as_deref()
                .expect("remote set when remote chosen");
            if dry_run {
                println!("projects: {}", selected.join(" "));
                println!("host:     remote ({host})");
                println!("command:  ssh {host} {}", attach::remote_command(primary));
                return Ok(());
            }
            attach::attach(&Remote {
                host,
                project: primary,
                extras,
                prompt,
                claude_args,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_args_shape() {
        let a = run_args("api", &["worker".into()], "hello", &["-p".into()]);
        assert_eq!(
            a,
            vec![
                "run",
                "--project",
                "api",
                "--add",
                "worker",
                "--prompt-b64",
                "aGVsbG8=",
                "--",
                "-p"
            ]
        );
    }

    #[test]
    fn run_args_omits_prompt_when_empty() {
        assert_eq!(
            run_args("api", &[], "", &[]),
            vec!["run", "--project", "api"]
        );
    }

    #[test]
    fn remote_command_wraps_login_shell() {
        // The serve invocation is wrapped so the remote login env (HORDE_*) is
        // loaded; the project is passed as the session key.
        let cmd = attach::remote_command("api");
        assert_eq!(cmd, "bash -lc 'horde-runner serve --project '\\''api'\\'''");
    }
}
