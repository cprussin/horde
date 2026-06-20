//! horde-runner argument parsing.  Three subcommands:
//!   run     — local direct launch (the old `horde-run` behaviour)
//!   serve   — per-connection relay spawned by ssh
//!   session — detached PTY session daemon
//!
//! `run` carries the full launch parameters; `serve`/`session` take only the
//! project (the socket key) — their launch parameters arrive in the protocol
//! `Hello` frame.

#[derive(Debug)]
pub enum Command {
    Run(RunArgs),
    Serve { project: String },
    Session { project: String },
    Help,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunArgs {
    pub project: String,
    pub extra_projects: Vec<String>,
    pub prompt_b64: String,
    pub dry_run: bool,
    pub claude_args: Vec<String>,
}

pub fn parse(argv: &[String]) -> Result<Command, String> {
    let (sub, rest) = match argv.split_first() {
        Some((s, r)) => (s.as_str(), r),
        None => return Err("missing subcommand (run|serve|session); see --help".to_string()),
    };
    match sub {
        "-h" | "--help" => Ok(Command::Help),
        "run" => Ok(Command::Run(parse_run(rest)?)),
        "serve" => Ok(Command::Serve {
            project: parse_project(rest, "serve")?,
        }),
        "session" => Ok(Command::Session {
            project: parse_project(rest, "session")?,
        }),
        other => Err(format!("unknown subcommand: {other} (run|serve|session)")),
    }
}

/// Parse the shared launch flags (a port of `horde-run.sh`'s argument loop).
fn parse_run(argv: &[String]) -> Result<RunArgs, String> {
    let mut a = RunArgs::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "-p" | "--project" => {
                a.project = take_value(argv, &mut i, arg)?;
            }
            "-a" | "--add" => {
                let v = take_value(argv, &mut i, arg)?;
                a.extra_projects.push(v);
            }
            "-P" | "--prompt-b64" => {
                a.prompt_b64 = take_value(argv, &mut i, arg)?;
            }
            "-n" | "--dry-run" => {
                a.dry_run = true;
                i += 1;
            }
            "--" => {
                a.claude_args = argv[i + 1..].to_vec();
                break;
            }
            other => return Err(format!("unexpected argument: {other} (see --help)")),
        }
    }
    if a.project.is_empty() {
        return Err("--project is required".to_string());
    }
    Ok(a)
}

fn parse_project(argv: &[String], sub: &str) -> Result<String, String> {
    let mut project = String::new();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "-p" | "--project" => project = take_value(argv, &mut i, arg)?,
            other => return Err(format!("unexpected argument for {sub}: {other}")),
        }
    }
    if project.is_empty() {
        return Err(format!("{sub} requires --project"));
    }
    Ok(project)
}

fn take_value(argv: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    let v = argv
        .get(*i + 1)
        .ok_or_else(|| format!("missing value for {flag}"))?
        .clone();
    *i += 2;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> RunArgs {
        match parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap() {
            Command::Run(r) => r,
            _ => panic!("expected run"),
        }
    }

    #[test]
    fn parses_run_flags() {
        let r = run(&[
            "run",
            "--project",
            "api",
            "--add",
            "worker",
            "-P",
            "aGk=",
            "--",
            "--resume",
        ]);
        assert_eq!(r.project, "api");
        assert_eq!(r.extra_projects, vec!["worker"]);
        assert_eq!(r.prompt_b64, "aGk=");
        assert_eq!(r.claude_args, vec!["--resume"]);
        assert!(!r.dry_run);
    }

    #[test]
    fn run_requires_project() {
        let err = parse(&["run".to_string(), "--dry-run".to_string()]).unwrap_err();
        assert_eq!(err, "--project is required");
    }

    #[test]
    fn serve_and_session_take_project() {
        match parse(&["serve".into(), "--project".into(), "api".into()]).unwrap() {
            Command::Serve { project } => assert_eq!(project, "api"),
            _ => panic!(),
        }
        match parse(&["session".into(), "-p".into(), "api".into()]).unwrap() {
            Command::Session { project } => assert_eq!(project, "api"),
            _ => panic!(),
        }
    }
}
