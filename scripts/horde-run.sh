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
  HORDE_GITHUB_TOKEN_FILES JSON object of GitHub owner -> token file.  The
                           "default" key is the host-level fallback (also
                           exported as GH_TOKEN/GITHUB_TOKEN); every other
                           key scopes its token to https://github.com/<owner>
                           via a generated git credential config, and a gh
                           wrapper picks the matching token by repo owner
  HORDE_GH_APP_ID          GitHub App ID; with HORDE_GH_APP_KEY_FILE, mints
                           installation tokens on demand per repo owner (a
                           fallback for any owner the App is installed on)
  HORDE_GH_APP_KEY_FILE    File with the GitHub App private key (PEM)
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
cfg_github_token_files="${HORDE_GITHUB_TOKEN_FILES:-}"
cfg_gh_app_id="${HORDE_GH_APP_ID:-}"
cfg_gh_app_key_file="${HORDE_GH_APP_KEY_FILE:-}"
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

# GitHub tokens, one per owner.  The "default" key becomes the host-level
# GH_TOKEN; every other owner's token is injected as HORDE_GH_TOKEN_<owner>
# and bound to that owner's repos via the git credential config below.
gh_owners=()
gh_owner_vars=()
have_gh_default=0
if [ -n "$cfg_github_token_files" ]; then
  while IFS= read -r entry; do
    [ -n "$entry" ] || continue
    owner="${entry%%=*}"
    file="${entry#*=}"
    [ -r "$file" ] || die "cannot read github token file: $file"
    token="$(cat "$file")"
    if [ "$owner" = default ]; then
      GH_TOKEN="$token"
      GITHUB_TOKEN="$token"
      export GH_TOKEN GITHUB_TOKEN
      keep_env+=(GH_TOKEN GITHUB_TOKEN)
      have_gh_default=1
    else
      case "$owner" in
        *[!A-Za-z0-9-]* | "" | -*)
          die "invalid github owner in HORDE_GITHUB_TOKEN_FILES: $owner"
          ;;
      esac
      # GitHub logins contain no underscores, so hyphen->underscore yields a
      # unique, valid environment-variable name.
      var="HORDE_GH_TOKEN_${owner//-/_}"
      export "$var=$token"
      keep_env+=("$var")
      gh_owners+=("$owner")
      gh_owner_vars+=("$var")
    fi
  done < <(jq -r 'to_entries[] | "\(.key)=\(.value)"' <<< "$cfg_github_token_files")
fi

# GitHub App: inject the App ID and private key so the credential helper can
# mint installation tokens on demand, per repo owner.
have_gh_app=0
if [ -n "$cfg_gh_app_id" ] && [ -n "$cfg_gh_app_key_file" ]; then
  [ -r "$cfg_gh_app_key_file" ] || die "cannot read github app key file: $cfg_gh_app_key_file"
  HORDE_GH_APP_ID="$cfg_gh_app_id"
  HORDE_GH_APP_KEY="$(cat "$cfg_gh_app_key_file")"
  export HORDE_GH_APP_ID HORDE_GH_APP_KEY
  keep_env+=(HORDE_GH_APP_ID HORDE_GH_APP_KEY)
  have_gh_app=1
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

# --- github credential config ---------------------------------------------

inner_path="${cfg_sandbox_path:+$cfg_sandbox_path:}$PATH"

# A managed include file holds the github credential helpers.  Helpers are
# tried in config order and the first to answer wins, so they are emitted
# most-specific-first: per-owner PATs, then the App (serves any installed
# owner, emits nothing otherwise), then the default PAT catch-all.  PAT
# values stay in the environment, referenced by name, never on disk.
if [ -n "$cfg_github_token_files" ] || [ "$have_gh_app" -eq 1 ]; then
  gh_conf="$state_dir/horde-github.gitconfig"
  {
    # useHttpPath makes git send the repo path, which the per-owner and App
    # helpers need to know the owner.
    if [ ${#gh_owners[@]} -gt 0 ] || [ "$have_gh_app" -eq 1 ]; then
      printf '[credential "https://github.com"]\n\tuseHttpPath = true\n'
    fi
    i=0
    while [ "$i" -lt ${#gh_owners[@]} ]; do
      printf '[credential "https://github.com/%s"]\n\thelper = "!f() { echo username=x-access-token; echo \\"password=$%s\\"; }; f"\n' \
        "${gh_owners[$i]}" "${gh_owner_vars[$i]}"
      i=$((i + 1))
    done
    if [ "$have_gh_app" -eq 1 ]; then
      gh_app_helper="$(command -v horde-gh-app-credential)"
      printf '[credential "https://github.com"]\n\thelper = "!%s"\n' "$gh_app_helper"
    fi
    if [ "$have_gh_default" -eq 1 ]; then
      # The $GH_TOKEN is meant to stay literal — it is expanded by the helper
      # shell inside the sandbox, not here.
      # shellcheck disable=SC2016
      printf '[credential "https://github.com"]\n\thelper = "!f() { echo username=x-access-token; echo \\"password=$GH_TOKEN\\"; }; f"\n'
      # shellcheck disable=SC2016
      printf '[credential "https://gist.github.com"]\n\thelper = "!f() { echo username=x-access-token; echo \\"password=$GH_TOKEN\\"; }; f"\n'
    fi
  } > "$gh_conf"

  gitconfig="$state_dir/.gitconfig"
  include_path="$sandbox_home/horde-github.gitconfig"
  if ! grep -qsF "$include_path" "$gitconfig"; then
    printf '[include]\n\tpath = %s\n' "$include_path" >> "$gitconfig"
  fi
fi

# gh takes a single token per host, ignoring git's per-path credentials, so
# when tokens vary by owner (per-owner PATs or the App) shadow gh with a
# wrapper that resolves the current repo's credential through git and sets
# GH_TOKEN before delegating.  Using `git credential fill` means it handles
# PATs and minted App tokens identically.  The wrapper strips its own
# directory from PATH so git/gh resolve to the session's real binaries.
if [ ${#gh_owners[@]} -gt 0 ] || [ "$have_gh_app" -eq 1 ]; then
  real_bash="$(command -v bash)"
  wrapper_dir="$sandbox_home/bin"
  mkdir -p "$state_dir/bin"
  cat > "$state_dir/bin/gh" << EOF
#!$real_bash
set -u
new_path=""
IFS=:
for d in \$PATH; do
  [ "\$d" = "$wrapper_dir" ] && continue
  new_path="\${new_path:+\$new_path:}\$d"
done
unset IFS
export PATH="\$new_path"
remote="\$(git remote get-url origin 2>/dev/null || true)"
case "\$remote" in
  *github.com[:/]*)
    rest="\${remote##*github.com}"
    rest="\${rest#[:/]}"
    rest="\${rest%.git}"
    owner="\${rest%%/*}"
    repo="\${rest#*/}"
    repo="\${repo%%/*}"
    ;;
  *) owner=""; repo="" ;;
esac
if [ -n "\$owner" ] && [ -n "\$repo" ]; then
  token="\$(printf 'protocol=https\nhost=github.com\npath=%s/%s\n\n' "\$owner" "\$repo" | git credential fill 2>/dev/null | sed -n 's/^password=//p' | head -n1)"
  if [ -n "\$token" ]; then
    GH_TOKEN="\$token"
    GITHUB_TOKEN="\$token"
    export GH_TOKEN GITHUB_TOKEN
  fi
fi
exec gh "\$@"
EOF
  chmod +x "$state_dir/bin/gh"
  inner_path="$wrapper_dir:$inner_path"
fi

# --- assemble the sandbox --------------------------------------------------

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
