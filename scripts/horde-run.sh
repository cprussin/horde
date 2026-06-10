usage() {
  cat << 'EOF'
horde-run - launch a sandboxed Claude Code session in a project

Runs claude --dangerously-skip-permissions with Claude Code's native OS
sandbox force-enabled via --settings, using the project directory as the
working directory (so its CLAUDE.md is auto-discovered).  Normally invoked
by horde, but usable directly.

Usage:
  horde-run --project <name> [options] [-- <extra args for claude>]

Options:
  -p, --project <name>    Project subdirectory to run in (required)
  -a, --add <name>        Additional project exposed via --add-dir
                          (repeatable)
  -P, --prompt-b64 <b64>  Initial prompt, base64-encoded
  -h, --help              Show this help

Environment:
  HORDE_PROJECTS         Projects directory (default: ~/Projects)
  HORDE_API_KEY_FILE     File whose contents are exported as
                         ANTHROPIC_API_KEY (for headless remote hosts)
  HORDE_CLAUDE_SETTINGS  JSON string or settings file path passed to claude
                         via --settings, replacing the built-in strict
                         sandbox settings
EOF
}

die() {
  echo "horde-run: $*" >&2
  exit 1
}

projects_dir="${HORDE_PROJECTS:-$HOME/Projects}"
project=""
extra_projects=()
prompt_b64=""
claude_args=()

while [ $# -gt 0 ]; do
  case "$1" in
    -p | --project)
      [ $# -ge 2 ] || die "missing value for $1"
      project="$2"
      shift 2
      ;;
    -a | --add)
      [ $# -ge 2 ] || die "missing value for $1"
      extra_projects+=("$2")
      shift 2
      ;;
    -P | --prompt-b64)
      [ $# -ge 2 ] || die "missing value for $1"
      prompt_b64="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      claude_args=("$@")
      break
      ;;
    *)
      die "unexpected argument: $1 (see --help)"
      ;;
  esac
done

check_name() {
  case "$1" in
    *[!A-Za-z0-9._-]* | "" | .*)
      die "invalid project name: $1"
      ;;
  esac
  [ -d "$projects_dir/$1" ] || die "no such project: $projects_dir/$1"
}

[ -n "$project" ] || die "--project is required"
check_name "$project"
for name in "${extra_projects[@]}"; do
  check_name "$name"
done

# The sandbox is the security perimeter that makes the bypass flag safe:
# fail hard if it can't start, and don't let commands retry outside it.
settings="${HORDE_CLAUDE_SETTINGS:-}"
if [ -z "$settings" ]; then
  settings='{"sandbox":{"enabled":true,"failIfUnavailable":true,"allowUnsandboxedCommands":false}}'
fi

if [ -n "${HORDE_API_KEY_FILE:-}" ]; then
  [ -r "$HORDE_API_KEY_FILE" ] || die "cannot read API key file: $HORDE_API_KEY_FILE"
  ANTHROPIC_API_KEY="$(cat "$HORDE_API_KEY_FILE")"
  export ANTHROPIC_API_KEY
fi

prompt=""
if [ -n "$prompt_b64" ]; then
  prompt="$(printf '%s' "$prompt_b64" | base64 -d)" || die "could not decode prompt"
fi

cd "$projects_dir/$project" || die "could not enter $projects_dir/$project"

cmd=(claude --dangerously-skip-permissions --settings "$settings")
for name in "${extra_projects[@]}"; do
  cmd+=(--add-dir "$projects_dir/$name")
done
cmd+=("${claude_args[@]}")
if [ -n "$prompt" ]; then
  cmd+=("$prompt")
fi

exec "${cmd[@]}"
