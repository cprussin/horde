usage() {
  cat << 'EOF'
horde - route a prompt to the right project and run Claude Code on it,
sandboxed, on this machine or a remote host

The prompt is matched against the projects in $HORDE_PROJECTS (default
~/Projects) by a headless Claude routing call, unless --project is given.
The session then runs via horde-run (Claude Code's native sandbox +
--dangerously-skip-permissions), either locally or on the remote host
depending on reachability and latency.

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
EOF
}

die() {
  echo "horde: $*" >&2
  exit 1
}

# Draw a Claude Code-style input box and read the initial prompt into the
# global $prompt.  The box is erased afterwards so the real session takes
# its place on screen.
read_prompt() {
  local cols border
  cols="$(stty size < /dev/tty 2> /dev/null | cut -d' ' -f2 || true)"
  case "$cols" in
    '' | *[!0-9]*) cols=80 ;;
  esac
  [ "$cols" -ge 20 ] || cols=80
  border="$(printf '%*s' $((cols - 2)) '' | tr ' ' '─')"

  # Box layout: top border / input row / bottom border / hint, then move
  # the cursor back up onto the input row to read there.
  printf '\033[2m╭%s╮\033[0m\n' "$border"
  printf '\n'
  printf '\033[2m╰%s╯\033[0m\n' "$border"
  printf '\033[2m  enter to launch · ctrl-c to cancel\033[0m\n'
  printf '\033[3A'

  # On ctrl-c, erase the box (cursor is still on the input row) and bail.
  trap 'printf "\r\033[1A\033[0J"; exit 130' INT

  while [ -z "$prompt" ]; do
    if ! IFS= read -e -r -p '│ > ' prompt; then
      # EOF (ctrl-d): erase the box and exit quietly.
      printf '\r\033[1A\033[0J'
      exit 0
    fi
    # Empty submission: readline moved us down a row; go back and re-read.
    [ -n "$prompt" ] || printf '\033[1A'
  done

  trap - INT
  # Enter left the cursor on the bottom border row; erase from the top
  # border down so the session starts where the box was.
  printf '\033[2A\033[0J'
}

projects_dir="${HORDE_PROJECTS:-$HOME/Projects}"
remote="${HORDE_REMOTE:-}"
latency_ms="${HORDE_LATENCY_MS:-150}"
connect_timeout="${HORDE_CONNECT_TIMEOUT:-2}"
router_model="${HORDE_ROUTER_MODEL:-claude-haiku-4-5}"
claude_token_file="${HORDE_CLAUDE_TOKEN_FILE:-}"

# The routing call runs outside the sandbox, so unlike the runner it has no
# token injected.  Authenticate it from the same configured token file, so a
# headless host needs no ambient Claude login.  An explicit token in the
# environment still wins.
authenticate_router() {
  [ -z "${CLAUDE_CODE_OAUTH_TOKEN:-}" ] || return 0
  [ -z "${ANTHROPIC_API_KEY:-}" ] || return 0
  [ -z "${ANTHROPIC_AUTH_TOKEN:-}" ] || return 0
  [ -n "$claude_token_file" ] || return 0
  [ -r "$claude_token_file" ] || die "cannot read claude token file: $claude_token_file"
  local token
  token="$(cat "$claude_token_file")"
  case "$token" in
    sk-ant-oat*) export CLAUDE_CODE_OAUTH_TOKEN="$token" ;;
    *) export ANTHROPIC_API_KEY="$token" ;;
  esac
}

projects_arg=""
force_host=""
dry_run=0
prompt_words=()
claude_args=()

while [ $# -gt 0 ]; do
  case "$1" in
    -p | --project)
      [ $# -ge 2 ] || die "missing value for $1"
      projects_arg="$2"
      shift 2
      ;;
    -H | --host)
      [ $# -ge 2 ] || die "missing value for $1"
      remote="$2"
      shift 2
      ;;
    -L | --local)
      force_host=local
      shift
      ;;
    -R | --remote)
      force_host=remote
      shift
      ;;
    -n | --dry-run)
      dry_run=1
      shift
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
    -*)
      die "unknown option: $1 (see --help)"
      ;;
    *)
      prompt_words+=("$1")
      shift
      ;;
  esac
done

prompt="${prompt_words[*]}"
[ -d "$projects_dir" ] || die "projects directory does not exist: $projects_dir"
if [ -z "$prompt" ] && [ -z "$projects_arg" ]; then
  if [ -t 0 ] && [ -t 1 ]; then
    read_prompt
  else
    die "no prompt given (a bare session needs an explicit --project)"
  fi
fi

check_name() {
  case "$1" in
    *[!A-Za-z0-9._-]* | "" | .*)
      die "invalid project name: $1"
      ;;
  esac
  [ -d "$projects_dir/$1" ] || die "no such project: $projects_dir/$1"
}

selected=()
if [ -n "$projects_arg" ]; then
  IFS=',' read -ra selected <<< "$projects_arg"
else
  # Catalog the projects from their CLAUDE.md headers, then ask a cheap
  # model which one(s) the prompt refers to.
  catalog=""
  while IFS= read -r dir; do
    name="$(basename "$dir")"
    desc="$(head -n 5 "$dir/CLAUDE.md" 2> /dev/null | tr '\n' ' ' || true)"
    catalog+="$name :: $desc"$'\n'
  done < <(find "$projects_dir" -mindepth 1 -maxdepth 1 -type d | sort)
  [ -n "$catalog" ] || die "no projects found in $projects_dir"

  routing_prompt="You route requests to projects.  Below is a list of \
projects in the form 'name :: description'.

Projects:
$catalog
Request: $prompt

Respond with ONLY a JSON array of the project directory names the request \
refers to, most relevant first, e.g. [\"foo\"] or [\"api\",\"worker\"].  Use \
the names exactly as listed.  If nothing matches, respond with []."

  authenticate_router
  # Keep stdout (the JSON) clean but capture stderr separately, so a failure
  # (auth, model, network) is shown rather than swallowed by the command
  # substitution.
  router_err="$(mktemp)"
  if ! router_output="$(claude --print --output-format json --model "$router_model" "$routing_prompt" 2> "$router_err")"; then
    router_msg="$(cat "$router_err")"
    rm -f "$router_err"
    die "routing call failed:"$'\n'"${router_msg:-$router_output}"
  fi
  rm -f "$router_err"
  router_text="$(jq -r '.result // empty' <<< "$router_output")"
  array_json="$(grep -o '\[.*\]' <<< "$router_text" | head -n 1 || true)"
  [ -n "$array_json" ] || die "router did not return a project list: $router_text"
  mapfile -t selected < <(jq -r '.[]' <<< "$array_json")
fi

[ ${#selected[@]} -gt 0 ] || die "no project matched the request; rerun with --project"
for name in "${selected[@]}"; do
  check_name "$name"
done

primary="${selected[0]}"
extras=("${selected[@]:1}")

pick_host() {
  if [ "$force_host" = local ]; then
    echo local
    return
  fi
  if [ -z "$remote" ]; then
    [ "$force_host" = remote ] && die "--remote given but no remote host is configured"
    echo local
    return
  fi
  if ! ssh -o BatchMode=yes -o ConnectTimeout="$connect_timeout" "$remote" true 2> /dev/null; then
    [ "$force_host" = remote ] && die "remote host $remote is not reachable"
    echo local
    return
  fi
  if [ "$force_host" = remote ]; then
    echo remote
    return
  fi
  # Latency proxy: time a full SSH round trip.  Note this reads near-zero
  # if a ControlMaster connection is already warm.
  local start end
  start="$(date +%s%3N)"
  ssh -o BatchMode=yes "$remote" true 2> /dev/null || {
    echo local
    return
  }
  end="$(date +%s%3N)"
  if [ $((end - start)) -le "$latency_ms" ]; then
    echo remote
  else
    echo local
  fi
}

host="$(pick_host)"

runner_args=(--project "$primary")
for name in "${extras[@]}"; do
  runner_args+=(--add "$name")
done
if [ -n "$prompt" ]; then
  # Base64 so the prompt survives the ssh/tmux quoting layers untouched.
  runner_args+=(--prompt-b64 "$(printf '%s' "$prompt" | base64 -w0)")
fi
if [ ${#claude_args[@]} -gt 0 ]; then
  runner_args+=(-- "${claude_args[@]}")
fi

if [ "$host" = local ]; then
  if [ "$dry_run" -eq 1 ]; then
    echo "projects: ${selected[*]}"
    echo "host:     local"
    echo "command:  horde-run${runner_args[*]:+ ${runner_args[*]}}"
    exit 0
  fi
  exec horde-run "${runner_args[@]}"
fi

runner_str="horde-run"
for arg in "${runner_args[@]}"; do
  runner_str+=" $(printf '%q' "$arg")"
done
session="horde-$primary"

# The runner's PATH and HORDE_* config come from the user's home-manager
# session variables, which are only loaded by a login shell — so run
# horde-run through `bash -lc`.  A second login shell wraps the whole tmux
# invocation so tmux itself is found on PATH, and the inner one re-applies
# the environment regardless of any already-running tmux server's env.
inner_login="bash -lc $(printf '%q' "$runner_str")"
# tmux -A attaches if the session already exists, so a dropped connection
# is recoverable by re-running the same command.
tmux_cmd="tmux new-session -A -s $(printf '%q' "$session") $(printf '%q' "$inner_login")"
remote_cmd="bash -lc $(printf '%q' "$tmux_cmd")"

if [ "$dry_run" -eq 1 ]; then
  echo "projects: ${selected[*]}"
  echo "host:     remote ($remote)"
  echo "command:  ssh -t $remote $remote_cmd"
  exit 0
fi
exec ssh -t "$remote" "$remote_cmd"
