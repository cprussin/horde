//! Command-line argument parsing.
//!
//! Hand-rolled rather than using a parser library so the option handling, the
//! `--` passthrough, and the bare-positional-as-prompt behaviour match the
//! original `horde.sh` exactly — including its error messages.

pub const USAGE: &str = "\
horde - route a prompt to the right project and run Claude Code on it,
sandboxed, on this machine or a remote host

The prompt is matched against the projects in $HORDE_PROJECTS (default
~/Projects) by a headless Claude routing call, unless --project is given.
The session is launched by horde-runner inside a strict sandbox: locally it
runs in this terminal; remotely it runs as a persistent session over ssh
whose IO is streamed here, so a dropped connection reattaches on the next run.

Usage:
  horde                  (prompts for input interactively)
  horde [options] <prompt>... [-- <extra args for claude>]
  horde --project <a[,b,...]> [options] [<prompt>...] [-- <extra args>]

Options:
  -p, --project <names>  Comma-separated project list; the first is the
                         working directory, the rest are exposed via
                         --add-dir.  Skips the routing call.
  -H, --host <target>    SSH destination to use as the remote host
                         (default: $HORDE_REMOTE)
  -L, --local            Force local execution
  -R, --remote           Force remote execution
  -n, --dry-run          Print the resolved projects, host, and command
                         without launching anything
  -h, --help             Show this help

Environment:
  HORDE_PROJECTS         Projects directory (default: ~/Projects)
  HORDE_REMOTE           SSH destination of the remote execution host;
                         unset means always run locally
  HORDE_LATENCY_MS       Max SSH round-trip in ms before falling back to
                         local execution (default: 150)
  HORDE_CONNECT_TIMEOUT  SSH reachability probe timeout in seconds
                         (default: 2)
  HORDE_ROUTER_MODEL     Model for the routing call
                         (default: claude-haiku-4-5)
  HORDE_CLAUDE_TOKEN_FILE  File with a Claude credential, used to
                         authenticate the routing call when no Claude token
                         is already in the environment
  HORDE_HISTORY_FILE     File storing interactive prompt history (default:
                         $XDG_STATE_HOME/horde/prompt-history)
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForceHost {
    Local,
    Remote,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Comma-separated `--project` value, split later.
    pub projects_arg: Option<String>,
    /// `--host` override for the remote destination.
    pub remote_override: Option<String>,
    pub force_host: Option<ForceHost>,
    pub dry_run: bool,
    pub help: bool,
    pub prompt_words: Vec<String>,
    /// Everything after `--`, passed through to claude verbatim.
    pub claude_args: Vec<String>,
}

pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "-p" | "--project" => {
                let v = argv
                    .get(i + 1)
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                args.projects_arg = Some(v.clone());
                i += 2;
            }
            "-H" | "--host" => {
                let v = argv
                    .get(i + 1)
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                args.remote_override = Some(v.clone());
                i += 2;
            }
            "-L" | "--local" => {
                args.force_host = Some(ForceHost::Local);
                i += 1;
            }
            "-R" | "--remote" => {
                args.force_host = Some(ForceHost::Remote);
                i += 1;
            }
            "-n" | "--dry-run" => {
                args.dry_run = true;
                i += 1;
            }
            "-h" | "--help" => {
                args.help = true;
                i += 1;
            }
            "--" => {
                args.claude_args = argv[i + 1..].to_vec();
                break;
            }
            s if s.starts_with('-') => {
                return Err(format!("unknown option: {s} (see --help)"));
            }
            _ => {
                args.prompt_words.push(arg.clone());
                i += 1;
            }
        }
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> Args {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn positional_words_become_prompt() {
        let a = parse_ok(&["fix", "the", "bug"]);
        assert_eq!(a.prompt_words, vec!["fix", "the", "bug"]);
        assert!(a.claude_args.is_empty());
    }

    #[test]
    fn double_dash_passthrough() {
        let a = parse_ok(&["-p", "a,b", "--", "--resume", "x"]);
        assert_eq!(a.projects_arg.as_deref(), Some("a,b"));
        assert_eq!(a.claude_args, vec!["--resume", "x"]);
        assert!(a.prompt_words.is_empty());
    }

    #[test]
    fn dash_p_after_double_dash_goes_to_claude() {
        let a = parse_ok(&["--", "-p"]);
        assert_eq!(a.claude_args, vec!["-p"]);
        assert!(a.projects_arg.is_none());
    }

    #[test]
    fn force_flags_and_overrides() {
        let a = parse_ok(&["-L", "-H", "me@host", "-n"]);
        assert_eq!(a.force_host, Some(ForceHost::Local));
        assert_eq!(a.remote_override.as_deref(), Some("me@host"));
        assert!(a.dry_run);
    }

    #[test]
    fn unknown_option_errors() {
        let err = parse(&["--bogus".to_string()]).unwrap_err();
        assert_eq!(err, "unknown option: --bogus (see --help)");
    }

    #[test]
    fn missing_value_errors() {
        let err = parse(&["--project".to_string()]).unwrap_err();
        assert_eq!(err, "missing value for --project");
    }
}
