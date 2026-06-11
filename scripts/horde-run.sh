usage() {
  cat << 'EOF'
horde-run - launch Claude Code in a strictly isolated project sandbox

Builds a bubblewrap namespace containing ONLY: /nix/store (read-only), the
selected project directories (read-write), a private persistent HOME, the
paths listed in the expose config, and minimal /etc plumbing (DNS, TLS
certs).  Nothing else on the host is visible.  Inside it, claude runs with
--dangerously-skip-permissions, with its native sandbox kept enabled as an
inner defense layer.  Secrets are injected as environment variables read
from the configured token files; all other environment variables are
scrubbed.

Usage:
  horde-run --project <name> [options] [-- <extra args for claude>]

Options:
  -p, --project <name>    Project subdirectory to run in (required)
  -a, --add <name>        Additional project exposed read-write and via
                          --add-dir (repeatable)
  -P, --prompt-b64 <b64>  Initial prompt, base64-encoded
  -n, --dry-run           Print the sandbox command without launching
  -h, --help              Show this help

Environment (normally set by the NixOS runner module):
  HORDE_PROJECTS           Projects directory (default: ~/Projects)
  HORDE_STATE_DIR          Host directory backing the sandbox HOME
                           (default: ~/.local/share/horde/home)
  HORDE_SANDBOX_PATH       PATH prefix for tools inside the sandbox
  HORDE_CLAUDE_TOKEN_FILE  File with a Claude credential; exported as
                           CLAUDE_CODE_OAUTH_TOKEN (sk-ant-oat...) or
                           ANTHROPIC_API_KEY (anything else)
  HORDE_GITHUB_TOKEN_FILE  File with a GitHub token; exported as
                           GH_TOKEN/GITHUB_TOKEN, and gh is wired up as
                           git's credential helper for github.com
  HORDE_TOKEN_FILES        JSON object of VAR -> file for other services
  HORDE_RO_PATHS           JSON array of extra read-only paths to expose
  HORDE_RW_PATHS           JSON array of extra read-write paths to expose
  HORDE_ALLOW_NIX          "1" to expose the nix daemon socket
  HORDE_CLAUDE_SETTINGS    JSON string or settings file path passed to
                           claude via --settings, replacing the built-in
                           strict inner-sandbox settings
EOF
}

die() {
  echo "horde-run: $*" >&2
  exit 1
}

# Capture all configuration up front: the environment is scrubbed before
# the sandbox launches, which also removes the HORDE_* variables.
projects_dir="${HORDE_PROJECTS:-$HOME/Projects}"
state_dir="${HORDE_STATE_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/horde/home}"
cfg_sandbox_path="${HORDE_SANDBOX_PATH:-}"
cfg_claude_token_file="${HORDE_CLAUDE_TOKEN_FILE:-}"
cfg_github_token_file="${HORDE_GITHUB_TOKEN_FILE:-}"
cfg_token_files="${HORDE_TOKEN_FILES:-}"
cfg_ro_paths="${HORDE_RO_PATHS:-}"
cfg_rw_paths="${HORDE_RW_PATHS:-}"
cfg_allow_nix="${HORDE_ALLOW_NIX:-}"
cfg_settings="${HORDE_CLAUDE_SETTINGS:-}"
user_name="${USER:-horde}"
if [ -z "$cfg_settings" ]; then
  # The inner perimeter: claude's native sandbox confines Bash commands and
  # routes their network through the domain-filtering proxy, nested inside
  # the outer bubblewrap namespace built below.
  cfg_settings='{"sandbox":{"enabled":true,"failIfUnavailable":true,"allowUnsandboxedCommands":false}}'
fi

project=""
extra_projects=()
prompt_b64=""
dry_run=0
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
project_path="$projects_dir/$project"

# --- secrets: read token files, export as environment variables -----------

keep_env=(PATH TERM COLORTERM LANG LC_ALL)

if [ -n "$cfg_claude_token_file" ]; then
  [ -r "$cfg_claude_token_file" ] || die "cannot read claude token file: $cfg_claude_token_file"
  token="$(cat "$cfg_claude_token_file")"
  case "$token" in
    sk-ant-oat*)
      export CLAUDE_CODE_OAUTH_TOKEN="$token"
      keep_env+=(CLAUDE_CODE_OAUTH_TOKEN)
      ;;
    *)
      export ANTHROPIC_API_KEY="$token"
      keep_env+=(ANTHROPIC_API_KEY)
      ;;
  esac
fi

if [ -n "$cfg_github_token_file" ]; then
  [ -r "$cfg_github_token_file" ] || die "cannot read github token file: $cfg_github_token_file"
  GH_TOKEN="$(cat "$cfg_github_token_file")"
  GITHUB_TOKEN="$GH_TOKEN"
  export GH_TOKEN GITHUB_TOKEN
  keep_env+=(GH_TOKEN GITHUB_TOKEN)
fi

if [ -n "$cfg_token_files" ]; then
  while IFS= read -r entry; do
    [ -n "$entry" ] || continue
    var="${entry%%=*}"
    file="${entry#*=}"
    case "$var" in
      *[!A-Za-z0-9_]* | "" | [0-9]*)
        die "invalid variable name in HORDE_TOKEN_FILES: $var"
        ;;
    esac
    [ -r "$file" ] || die "cannot read token file: $file"
    value="$(cat "$file")"
    export "$var=$value"
    keep_env+=("$var")
  done < <(jq -r 'to_entries[] | "\(.key)=\(.value)"' <<< "$cfg_token_files")
fi

# Scrub everything else so no host environment leaks into the sandbox.
# Secrets stay in the (exported) environment rather than going through
# bwrap's argv, so they are never visible in /proc/<pid>/cmdline.  Names are
# read from `export -p`; values spanning multiple lines are safe because
# only the leading `declare -x NAME=` line matches.
while IFS= read -r var; do
  [ -n "$var" ] || continue
  keep=0
  for k in "${keep_env[@]}"; do
    if [ "$var" = "$k" ]; then
      keep=1
      break
    fi
  done
  if [ "$keep" -eq 0 ]; then
    unset "$var"
  fi
done < <(export -p | sed -n 's/^declare -x \([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p')

# --- persistent sandbox home ----------------------------------------------

sandbox_home=/home/horde
mkdir -p "$state_dir"

# With a GitHub token present, wire gh up as git's credential helper once,
# so HTTPS pushes work without any per-session auth.
if [ -n "${GH_TOKEN:-}" ]; then
  gitconfig="$state_dir/.gitconfig"
  if ! grep -qs 'gh auth git-credential' "$gitconfig"; then
    cat >> "$gitconfig" << 'EOF'
[credential "https://github.com"]
	helper = "!gh auth git-credential"
[credential "https://gist.github.com"]
	helper = "!gh auth git-credential"
EOF
  fi
fi

# --- assemble the sandbox --------------------------------------------------

inner_path="${cfg_sandbox_path:+$cfg_sandbox_path:}$PATH"

bwrap_args=(
  --die-with-parent
  --unshare-all
  --share-net
  --proc /proc
  --dev /dev
  --tmpfs /tmp
  --ro-bind /nix/store /nix/store
  --bind "$state_dir" "$sandbox_home"
  --bind "$project_path" "$project_path"
  --chdir "$project_path"
  --setenv HOME "$sandbox_home"
  --setenv USER "$user_name"
  --setenv XDG_RUNTIME_DIR /tmp
  --setenv PATH "$inner_path"
)

for name in "${extra_projects[@]}"; do
  bwrap_args+=(--bind "$projects_dir/$name" "$projects_dir/$name")
done

# Minimal /etc plumbing: DNS, TLS certs, NSS, timezone.  /etc/static is the
# NixOS indirection target many of these symlink through.
for path in \
  /etc/resolv.conf \
  /etc/ssl \
  /etc/static \
  /etc/hosts \
  /etc/nsswitch.conf \
  /etc/passwd \
  /etc/group \
  /etc/localtime \
  /etc/machine-id \
  /bin/sh \
  /usr/bin/env; do
  if [ -e "$path" ]; then
    bwrap_args+=(--ro-bind "$path" "$path")
  fi
done

if [ -e /etc/ssl/certs/ca-certificates.crt ]; then
  bwrap_args+=(
    --setenv SSL_CERT_FILE /etc/ssl/certs/ca-certificates.crt
    --setenv NIX_SSL_CERT_FILE /etc/ssl/certs/ca-certificates.crt
  )
fi

case "$cfg_allow_nix" in
  1 | true | yes)
    if [ -e /nix/var/nix/daemon-socket ]; then
      bwrap_args+=(--bind /nix/var/nix/daemon-socket /nix/var/nix/daemon-socket)
    fi
    if [ -e /etc/nix ]; then
      bwrap_args+=(--ro-bind /etc/nix /etc/nix)
    fi
    # Force the daemon store.  Inside a user namespace with a read-only
    # /nix/store, nix's `auto` store otherwise spins up a private chroot
    # store under HOME and re-fetches the whole closure.  With the daemon,
    # builds and substitutions land in the real store and become visible
    # through the live /nix/store bind mount.  (nix is added to the sandbox
    # PATH by the home-manager module when allowNix is set.)
    bwrap_args+=(--setenv NIX_REMOTE daemon)
    ;;
esac

if [ -n "$cfg_ro_paths" ]; then
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    [ -e "$path" ] || die "exposed read-only path does not exist: $path"
    bwrap_args+=(--ro-bind "$path" "$path")
  done < <(jq -r '.[]' <<< "$cfg_ro_paths")
fi

if [ -n "$cfg_rw_paths" ]; then
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    [ -e "$path" ] || die "exposed read-write path does not exist: $path"
    bwrap_args+=(--bind "$path" "$path")
  done < <(jq -r '.[]' <<< "$cfg_rw_paths")
fi

# --- launch -----------------------------------------------------------------

prompt=""
if [ -n "$prompt_b64" ]; then
  prompt="$(printf '%s' "$prompt_b64" | base64 -d)" || die "could not decode prompt"
fi

cmd=(claude --dangerously-skip-permissions --settings "$cfg_settings")
for name in "${extra_projects[@]}"; do
  cmd+=(--add-dir "$projects_dir/$name")
done
cmd+=("${claude_args[@]}")
if [ -n "$prompt" ]; then
  cmd+=("$prompt")
fi

if [ "$dry_run" -eq 1 ]; then
  printf '%q ' bwrap "${bwrap_args[@]}" "${cmd[@]}"
  echo
  exit 0
fi

exec bwrap "${bwrap_args[@]}" "${cmd[@]}"
