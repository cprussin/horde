# horde-gh-app-credential - git credential helper that mints GitHub App
# installation tokens on demand, scoped to the requested repo's owner.
#
# Configured by horde-run as a git credential helper for github.com (with
# useHttpPath on, so git passes the repo path).  It reads the App ID and
# private key from the environment, finds the App's installation for the
# requested owner, mints a short-lived installation token, and returns it.
#
# If the App is not installed for the owner (or anything goes wrong) it emits
# nothing and exits 0, so git falls through to the next configured helper
# (e.g. a per-owner or default PAT).
#
# Environment:
#   HORDE_GH_APP_ID   GitHub App ID
#   HORDE_GH_APP_KEY  GitHub App private key (PEM contents)

# Only the `get` operation needs a credential.
[ "${1:-}" = get ] || exit 0
[ -n "${HORDE_GH_APP_ID:-}" ] || exit 0
[ -n "${HORDE_GH_APP_KEY:-}" ] || exit 0

# Read the credential request from git on stdin.
host=""
path=""
while IFS='=' read -r key value; do
  [ -n "$key" ] || break
  case "$key" in
    host) host="$value" ;;
    path) path="$value" ;;
  esac
done

[ "$host" = github.com ] || exit 0
[ -n "$path" ] || exit 0

path="${path%.git}"
owner="${path%%/*}"
repo="${path#*/}"
repo="${repo%%/*}"
[ -n "$owner" ] || exit 0
[ -n "$repo" ] && [ "$repo" != "$owner" ] || repo=""

emit() {
  echo "username=x-access-token"
  echo "password=$1"
  exit 0
}

now="$(date +%s)"

# Per-owner token cache in tmpfs; tokens last an hour, reuse while fresh.
cache_dir="${XDG_RUNTIME_DIR:-/tmp}/horde-ghapp"
mkdir -p "$cache_dir" 2> /dev/null || true
chmod 700 "$cache_dir" 2> /dev/null || true
cache="$cache_dir/$owner"
if [ -r "$cache" ]; then
  cached_token="$(sed -n 1p "$cache")"
  cached_exp="$(sed -n 2p "$cache")"
  case "$cached_exp" in
    '' | *[!0-9]*) cached_exp=0 ;;
  esac
  if [ -n "$cached_token" ] && [ "$((cached_exp - now))" -gt 120 ]; then
    emit "$cached_token"
  fi
fi

# Build a short-lived App JWT (RS256) signed with the private key.
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

key_file="$(mktemp)" || exit 0
chmod 600 "$key_file"
trap 'rm -f "$key_file"' EXIT
printf '%s\n' "$HORDE_GH_APP_KEY" > "$key_file"

header="$(printf '%s' '{"alg":"RS256","typ":"JWT"}' | b64url)"
payload="$(printf '{"iat":%d,"exp":%d,"iss":"%s"}' "$((now - 60))" "$((now + 540))" "$HORDE_GH_APP_ID" | b64url)"
signing_input="$header.$payload"
signature="$(printf '%s' "$signing_input" | openssl dgst -sha256 -sign "$key_file" -binary 2> /dev/null | b64url)" || exit 0
[ -n "$signature" ] || exit 0
jwt="$signing_input.$signature"

api() {
  curl -fsS \
    -H "Authorization: Bearer $jwt" \
    -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "$@" 2> /dev/null
}

# Find the App's installation that owns this repo, then mint a token for it.
if [ -n "$repo" ]; then
  installation="$(api "https://api.github.com/repos/$owner/$repo/installation")" || installation=""
else
  installation="$(api "https://api.github.com/orgs/$owner/installation")" || installation=""
  [ -n "$installation" ] || installation="$(api "https://api.github.com/users/$owner/installation")" || installation=""
fi
[ -n "$installation" ] || exit 0

installation_id="$(printf '%s' "$installation" | jq -r '.id // empty')"
[ -n "$installation_id" ] || exit 0

token_response="$(api -X POST "https://api.github.com/app/installations/$installation_id/access_tokens")" || exit 0
token="$(printf '%s' "$token_response" | jq -r '.token // empty')"
expires_at="$(printf '%s' "$token_response" | jq -r '.expires_at // empty')"
[ -n "$token" ] || exit 0

if [ -n "$expires_at" ]; then
  exp_epoch="$(date -d "$expires_at" +%s 2> /dev/null || echo 0)"
  umask 077
  printf '%s\n%s\n' "$token" "$exp_epoch" > "$cache" 2> /dev/null || true
fi

emit "$token"
