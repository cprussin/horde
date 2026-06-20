//! Local direct launch: build the sandbox and exec bwrap+claude on the
//! inherited terminal (the old `horde-run` behaviour).  Also provides the
//! shared sandbox preparation used by the remote session daemon.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{bail, Result};

use crate::bwrap::{self, Sandbox, TermEnv};
use crate::cli::RunArgs;
use crate::config::{check_name, Config};
use crate::secrets;
use crate::util::shell_word;

/// Validate the projects and assemble the sandbox (shared by `run` and the
/// remote `session` supervisor).
pub fn prepare(config: &Config, args: &RunArgs, term: &TermEnv) -> Result<Sandbox> {
    check_name(&config.projects_dir, &args.project)?;
    for name in &args.extra_projects {
        check_name(&config.projects_dir, name)?;
    }
    let secrets = secrets::collect(config)?;
    bwrap::assemble(config, args, &secrets, term)
}

/// Build a `Command` that runs the sandbox with the prepared environment.
pub fn command(sandbox: &Sandbox) -> Command {
    let mut cmd = Command::new("bwrap");
    cmd.args(&sandbox.bwrap_args).args(&sandbox.command);
    cmd.env_clear();
    cmd.envs(sandbox.child_env.iter().cloned());
    cmd
}

/// The `run` subcommand: prepare and exec in this terminal (or print dry-run).
pub fn run(config: &Config, args: &RunArgs) -> Result<()> {
    let sandbox = prepare(config, args, &TermEnv::from_env())?;

    if args.dry_run {
        let mut line = String::from("bwrap");
        for arg in sandbox.bwrap_args.iter().chain(sandbox.command.iter()) {
            line.push(' ');
            line.push_str(&shell_word(arg));
        }
        println!("{line}");
        return Ok(());
    }

    let err = command(&sandbox).exec();
    bail!("failed to exec bwrap: {err}");
}
