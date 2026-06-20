//! horde — route a prompt to the right project and run Claude Code on it,
//! sandboxed, locally or on a remote host.  A Rust port of the original
//! `horde.sh` client, with a ratatui interactive prompt.

mod app;
mod attach;
mod cli;
mod config;
mod discovery;
mod host;
mod keys;
mod quote;
mod router;
mod session_conn;
mod tui;
mod validate;

use std::io::IsTerminal;
use std::process;

use anyhow::{bail, Result};

use app::{App, Initial};
use cli::Args;
use config::Config;
use discovery::Host;
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

    let remotes = config.discovery_remotes();
    let prompt = args.prompt_words.join(" ");
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    // Bare invocation (no prompt, no --project): open the session switcher.
    if prompt.is_empty() && args.projects_arg.is_none() {
        if !interactive {
            bail!("no prompt given (a bare session needs an explicit --project)");
        }
        return App::new(config, remotes).run(None);
    }

    // Otherwise resolve which project(s) and where, and create/focus a session.
    let selected = select_projects(&args, &config, &prompt)?;
    if selected.is_empty() {
        bail!("no project matched the request; rerun with --project");
    }
    for name in &selected {
        validate::check_name(&config.projects_dir, name)?;
    }
    let host = match host::pick_host(&config, args.force_host)? {
        Decision::Local => Host::Local,
        Decision::Remote => Host::Remote(config.remote.clone().unwrap_or_default()),
    };

    if args.dry_run {
        println!("projects: {}", selected.join(" "));
        println!("host:     {}", host.label());
        println!(
            "command:  {}",
            session_conn::dry_run_command(&host, &selected[0])
        );
        return Ok(());
    }

    if interactive {
        App::new(config, remotes).run(Some(Initial {
            projects: selected,
            prompt,
            claude_args: args.claude_args,
            host,
        }))
    } else {
        // No TTY for the switcher: stream the single session directly.
        let extras = selected[1..].to_vec();
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let hello = session_conn::make_hello(
            &selected[0],
            &extras,
            &prompt,
            &args.claude_args,
            cols,
            rows,
        );
        let code = attach::stream_oneshot(&host, hello)?;
        process::exit(code);
    }
}

/// Either split an explicit `--project` list or run the routing call.
fn select_projects(args: &Args, config: &Config, prompt: &str) -> Result<Vec<String>> {
    if let Some(arg) = &args.projects_arg {
        return Ok(arg.split(',').map(str::to_string).collect());
    }
    router::route(config, prompt)
}
