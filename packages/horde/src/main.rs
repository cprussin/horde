//! horde — route a prompt to the right project and run Claude Code on it,
//! sandboxed, locally or on a remote host.  A Rust port of the original
//! `horde.sh` client, with a ratatui interactive prompt.

mod attach;
mod cli;
mod config;
mod dispatch;
mod host;
mod quote;
mod router;
mod tui;
mod validate;

use std::io::IsTerminal;
use std::process;

use anyhow::{bail, Result};

use cli::Args;
use config::Config;
use host::Decision;

fn main() {
    if let Err(err) = run() {
        eprintln!("horde: {err:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match cli::parse(&argv) {
        Ok(args) => args,
        // Match bash's `die`: "horde: <msg>" on stderr, exit 1.
        Err(msg) => {
            eprintln!("horde: {msg}");
            process::exit(1);
        }
    };
    if args.help {
        print!("{}", cli::USAGE);
        return Ok(());
    }

    let mut config = Config::from_env();
    if let Some(host) = &args.remote_override {
        config.remote = Some(host.clone());
    }

    if !config.projects_dir.is_dir() {
        bail!(
            "projects directory does not exist: {}",
            config.projects_dir.display()
        );
    }

    let prompt = resolve_prompt(&args, &config)?;
    let routed = args.projects_arg.is_none();

    let selected = select_projects(&args, &config, &prompt, routed)?;
    if selected.is_empty() {
        bail!("no project matched the request; rerun with --project");
    }
    for name in &selected {
        validate::check_name(&config.projects_dir, name)?;
    }
    if routed {
        status(&format!("matched: {}", selected.join(", ")));
    }

    let decision = host::pick_host(&config, args.force_host)?;
    if !args.dry_run {
        match &decision {
            Decision::Local => status("host: local"),
            Decision::Remote => status(&format!(
                "host: remote ({})",
                config.remote.as_deref().unwrap_or("")
            )),
        }
    }

    dispatch::dispatch(
        &config,
        &selected,
        &prompt,
        &args.claude_args,
        decision,
        args.dry_run,
    )
}

/// Resolve the prompt from CLI words, or prompt interactively.  A bare session
/// (an explicit `--project` with no prompt) is allowed; a non-interactive
/// invocation with neither is an error.
fn resolve_prompt(args: &Args, config: &Config) -> Result<String> {
    let prompt = args.prompt_words.join(" ");
    if !prompt.is_empty() || args.projects_arg.is_some() {
        return Ok(prompt);
    }
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        match tui::read_prompt(&config.history_file)? {
            Some(p) => Ok(p),
            None => process::exit(0), // EOF / cancel from the input box
        }
    } else {
        bail!("no prompt given (a bare session needs an explicit --project)");
    }
}

/// Either split an explicit `--project` list or run the routing call.
fn select_projects(
    args: &Args,
    config: &Config,
    prompt: &str,
    routed: bool,
) -> Result<Vec<String>> {
    if let Some(arg) = &args.projects_arg {
        return Ok(arg.split(',').map(str::to_string).collect());
    }
    if routed {
        status("routing…");
    }
    router::route(config, prompt)
}

/// Emit a live status line ("routing…", "matched: …", "host: …") to stderr
/// when it's a terminal, so it's visible before the session takes over the
/// screen but stays out of any piped stdout.
fn status(msg: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("  {msg}");
    }
}
