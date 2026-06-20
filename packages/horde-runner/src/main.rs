//! horde-runner — sandboxed Claude Code launcher and PTY-streaming session
//! service.  Subcommands: `run` (local direct), `serve` (per-connection
//! relay), `session` (detached supervisor).

mod bwrap;
mod cli;
mod config;
mod ghwrapper;
mod gitconfig;
mod run;
mod runtime;
mod secrets;
mod serve;
mod session;
mod util;

use std::process;

use anyhow::Result;

use config::Config;

const USAGE: &str = "\
horde-runner - launch Claude Code in a strictly isolated project sandbox

Usage:
  horde-runner run     --project <name> [--add <name>]... [--prompt-b64 <b64>]
                       [--dry-run] [-- <extra args for claude>]
  horde-runner serve   --project <name>   (per-connection relay; reads a Hello
                                           frame on stdin, streams the session)
  horde-runner session --project <name>   (detached PTY session daemon)

Run builds a bubblewrap namespace containing only /nix/store (read-only), the
selected project directories (read-write), a private persistent HOME, the
configured expose paths, and minimal /etc plumbing, then runs claude with
--dangerously-skip-permissions inside it.  Secrets are read from the
configured token files and injected as environment variables; all other
environment variables are scrubbed.

serve/session implement the remote streaming model: the client runs
`ssh <host> horde-runner serve --project <p>`, which attaches to (or spawns)
a persistent `session` daemon that owns the claude PTY, so a dropped
connection can reattach.

Configuration comes from the HORDE_* environment (normally set by the
home-manager module): HORDE_PROJECTS, HORDE_STATE_DIR, HORDE_SANDBOX_PATH,
HORDE_CLAUDE_TOKEN_FILE, HORDE_GITHUB_TOKEN_FILES, HORDE_GH_APP_ID,
HORDE_GH_APP_KEY_FILE, HORDE_TOKEN_FILES, HORDE_RO_PATHS, HORDE_RW_PATHS,
HORDE_ALLOW_NIX, HORDE_CLAUDE_SETTINGS.
";

fn main() {
    if let Err(err) = real_main() {
        eprintln!("horde-runner: {err:#}");
        process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = cli::parse(&argv).map_err(|m| anyhow::anyhow!(m))?;
    match command {
        cli::Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        cli::Command::Run(args) => run::run(&Config::from_env(), &args),
        cli::Command::Serve { project } => serve::serve(&Config::from_env(), &project),
        cli::Command::Session { project } => session::run(&Config::from_env(), &project),
    }
}
