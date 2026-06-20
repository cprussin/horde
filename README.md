# horde

Run Claude Code tasks against your projects from anywhere, sandboxed, on
whichever machine is best.

You type a prompt; horde figures out which project in `~/Projects` you mean
(using a cheap headless Claude routing call against each project's
`CLAUDE.md`), picks a host (local, or a remote server if it's reachable with
low latency), and launches `claude --dangerously-skip-permissions` inside a
strictly isolated sandbox containing **only** the selected project(s) and an
explicit allowlist of paths — nothing else on the host is visible.

Project files are assumed to already exist at the same path on both machines
(e.g. synced with Syncthing); there is no sync layer.

## Components

| Piece       | Runs on | Job                                                                  |
| ----------- | ------- | -------------------------------------------------------------------- |
| `horde`     | client  | Catalog projects, route the prompt, gate local/remote, hand off      |
| `horde-run` | both    | Build the isolation sandbox, inject secrets, launch the session      |

The remote host never sees the router or any project-selection logic.  Its
entire footprint is `horde-run` (which carries `claude-code`, `bubblewrap`,
and `socat` with it), the user-namespaces sysctl, and whatever SSH access you
already have to it.  `tmux` (used to wrap the session) is referenced by
absolute store path via `HORDE_TMUX`, so it needs no install on the remote's
PATH.

## Isolation model

Every session — local or remote — runs in two nested layers:

1. **Outer namespace (horde-run's own bubblewrap).** A mount + PID + user
   namespace whose filesystem is built from an explicit allowlist:
   - `/nix/store` (read-only) and a minimal set of `/etc` files needed for
     DNS and TLS;
   - the selected project directory, and any `--add` projects (read-write);
   - a private persistent HOME (`stateDir`) for Claude Code's own state;
   - whatever you list in `exposeReadOnly` / `exposeReadWrite`.

   Nothing else is mounted, so Claude — including its Read tool, which
   `--dangerously-skip-permissions` would otherwise let read any file —
   cannot see your home directory, `~/.ssh`, other projects, `/etc/shadow`,
   or any host path you didn't list.  The environment is scrubbed to a
   minimal allowlist plus the injected secrets, so no host variables leak
   in.

2. **Inner sandbox (Claude Code's native sandbox).** Kept enabled inside
   the outer namespace as defense in depth: it confines the Bash tool and
   routes its network through the hostname-filtering proxy.

Secrets are never embedded in the image or the command line.  They are read
from the token files you configure and injected as environment variables
inside the sandbox (so they appear only in the process environment, not in
`/proc/<pid>/cmdline`).

## Installation

horde splits across two modules by what each setting actually requires:

- a single **NixOS module** (`nixosModules.default`) for the only thing that
  must be system-level: the unprivileged user-namespaces sysctl bubblewrap
  depends on.  Enable it with `programs.horde.enable` on any machine that
  runs sessions (client or server).  It does **not** force sshd — a remote
  server is assumed to already have SSH (or another tunnel) set up.
- a **home-manager module** (`homeManagerModules.default`, also exported as
  `homeModules.default`) for everything user-space — the packages and all
  configuration.  This is where you pick the role with
  `programs.horde.client.enable` / `programs.horde.server.enable`.

```nix
# NixOS config — the one system requirement (client and server alike):
{
  imports = [ inputs.horde.nixosModules.default ];
  programs.horde.enable = true;
}
```

```nix
# home-manager config — packages, role, router, sandbox, and secrets:
{
  imports = [ inputs.horde.homeManagerModules.default ];

  programs.horde.client = {
    enable = true;
    remote = "me@server.lan";            # omit to always run locally
  };
  programs.horde.runner = {
    # Secrets, deployed out-of-store via sops-nix/agenix:
    claudeTokenFile = "/run/secrets/claude-token";
    githubTokenFiles.default = "/run/secrets/github-token";
  };
}
```

On the server, set `programs.horde.enable = true` (NixOS) and
`programs.horde.server.enable = true` (home-manager), and give the
home-manager `programs.horde.runner` the same secret files.  A machine that
plays both roles enables both home-manager options.

The `programs.horde.runner` options (the sandbox and secrets) apply on
whichever machine actually runs sessions — the client for local runs, the
server for remote ones.  Because the configuration lives in home-manager
session variables, the remote handoff runs through a login shell so they
load; this assumes home-manager manages the worker user's shell (the usual
setup).

The packages are also exposed directly as `packages.<system>.horde` and
`packages.<system>.horde-run`, and as overlays, if you'd rather wire things
up yourself.  The flake instantiates its own nixpkgs with the `claude-code`
unfree exception, so you don't need to touch your system's `allowUnfree`
settings.

### Runner options

All under `programs.horde.runner`:

| Option            | Meaning                                                                 |
| ----------------- | ----------------------------------------------------------------------- |
| `projectsDir`     | Directory holding the projects (default `~/Projects`)                    |
| `stateDir`        | Host dir backing the sandbox HOME (default `~/.local/share/horde/home`)  |
| `claudeTokenFile` | File with a Claude credential (see below)                               |
| `githubTokenFiles`| GitHub PATs keyed by owner; `default` is the fallback (see below)        |
| `githubApp`       | GitHub App (`appId` + `privateKeyFile`); mints per-org tokens on demand  |
| `extraTokenFiles` | `{ VAR = "/path"; }` — other secrets, exported under the given names     |
| `exposeReadOnly`  | Extra host paths mounted read-only in the sandbox                       |
| `exposeReadWrite` | Extra host paths mounted read-write in the sandbox                      |
| `packages`        | Tools available on PATH inside the sandbox (sensible default set)        |
| `allowNix`        | Let sessions use nix (`nix develop`/direnv) inside the sandbox (default `false`) |
| `claudeSettings`  | Override the inner native-sandbox `--settings` (e.g. egress allowlist)   |

### Nix dev shells inside the sandbox

A common project layout has Claude create a git worktree mid-session and
only *then* enter a per-repo `nix develop` shell — so the dev environment
can't be resolved up front, it has to work from inside the sandbox.

Set `programs.horde.runner.allowNix = true` and it does.  The sandbox's
`/nix/store` is read-only (as on any system — only the daemon writes to it),
so horde exposes the nix daemon socket and `/etc/nix`, puts `nix` on the
sandbox PATH, and sets `NIX_REMOTE=daemon`.  That last part matters: inside
a user namespace with a read-only store, nix's `auto` store otherwise builds
a throwaway private store under HOME and refetches the whole closure;
forcing the daemon makes builds and substitutions land in the real store,
which then show up live through the read-only bind mount.  Worktrees,
`nix develop`, and direnv all work, because the project (including `.git`)
is mounted read-write.

The cost is that the session can drive the daemon to realize arbitrary store
paths and consume build resources — hence it is opt-in.  If you also lock
egress with `claudeSettings`, allow the substituter/cache domains nix needs
(`cache.nixos.org`, `*.cachix.org`, and `github.com` / `api.github.com` for
flake inputs).

### Authentication

Provide credentials as files (deploy them out of the nix store with
sops-nix/agenix, mode `0600`); nothing needs an interactive login per
session.

- **Claude** — `claudeTokenFile`.  Run `claude setup-token` once to mint a
  long-lived OAuth token (starts with `sk-ant-oat`); horde exports it as
  `CLAUDE_CODE_OAUTH_TOKEN`.  Any other value is treated as an API key and
  exported as `ANTHROPIC_API_KEY`.  This authenticates both the sandboxed
  session and the router's project-selection call, so a headless host needs
  no `claude` login (an explicit Claude token already in the environment
  still takes precedence for the router).
- **GitHub** — either a **GitHub App** (`githubApp`, recommended once you
  span more than a couple of orgs) or per-owner **PATs** (`githubTokenFiles`),
  or both.  See [Multiple GitHub organizations](#multiple-github-organizations).
  git push/pull and `gh` work with no per-session auth.
- **Anything else** — `extraTokenFiles`, e.g.
  `{ CACHIX_AUTH_TOKEN = "/run/secrets/cachix"; }`.

Because the token files are the *only* credentials mounted, this is a strict
allowlist: a service is reachable iff you gave horde its token.  No host
credential store (`~/.ssh`, `~/.config/gh`, `~/.netrc`) is visible to the
agent at all.

Caveat: **use HTTPS git remotes inside workers, not SSH.**  Sandboxed
network traffic goes through a hostname-based HTTP(S) proxy; git-over-SSH
(port 22) doesn't pass through it.  HTTPS+token (configured above) is also
the safer choice given the bypass-permissions agent.  If you lock egress
with `claudeSettings.sandbox.network.allowedDomains`, include each service's
domains — GitHub needs `github.com`, `api.github.com`, `codeload.github.com`,
and `objects.githubusercontent.com`.

### Multiple GitHub organizations

Projects spanning several orgs need a separately-scoped credential per org
(fine-grained PATs and GitHub Apps are both scoped to a single org's
resources).  horde supports two mechanisms, which can be combined.

#### GitHub App (recommended)

A GitHub App replaces N expiring PATs with **one** durable secret (the App
private key).  Permissions are defined once on the App; per-use it mints a
short-lived (1 h) installation token scoped to the requested org, so there's
nothing to rotate, and adding an org is just installing the App on it — no
horde config change.

```nix
programs.horde.runner.githubApp = {
  appId          = 123456;
  privateKeyFile = "/run/secrets/github-app-key.pem";   # sops-nix/agenix
};
```

Setup: create a GitHub App (Settings → Developer settings → GitHub Apps),
give it the repository permissions your agents need (Contents + Pull requests
+ Metadata is a sensible base), generate a private key, and install it on
each org.  horde wires a git credential helper that, on demand, signs an App
JWT and exchanges it for an installation token for the repo's owner (cached
in tmpfs for the token's lifetime).  If the App isn't installed for an owner,
the helper falls through to whatever you configured below.

#### Per-owner PATs

Give each owner its own fine-grained, repo-scoped token file:

```nix
programs.horde.runner.githubTokenFiles = {
  default   = "/run/secrets/github-default";  # fallback for any other owner
  acme-corp = "/run/secrets/github-acme";
  side-org  = "/run/secrets/github-side";
};
```

Each token is bound to `https://github.com/<owner>` in a generated git
credential config (via git's `useHttpPath`); tokens live only in the
environment, never in the on-disk gitconfig.

#### How they combine

git tries credentials most-specific-first: a per-owner PAT, then the App
(for any owner it's installed on), then `githubTokenFiles.default`.  So you
can run the App for most orgs and still pin a specific PAT for one owner, or
keep a default PAT as the catch-all.  Because `gh` takes a single token per
host rather than per path, whenever credentials vary by owner horde also
installs a `gh` wrapper that resolves the current repo's credential through
git (covering both PATs and minted App tokens) before delegating to `gh`.

## Usage

```bash
horde                                 # opens an input box; type your prompt
horde "add retry logic to the upload client in my filesync project"
horde --project api,worker "thread the new auth token through both services"
horde --local "quick scratch edit in the blog project"
horde --remote --project api "run the full integration suite and fix failures"
horde --project blog                  # open a bare session in a project
horde --dry-run "where would this go?"
horde --project api "summarize TODOs" -- -p   # non-interactive print mode
```

- Bare `horde` on a terminal draws a Claude Code-style input box, reads
  your prompt, then hands off — the box is erased and replaced by the real
  session.
- `--project a,b,…` skips the router; the first project becomes the working
  directory and the rest are exposed via `--add-dir`.
- With no `--project`, a headless call to `$HORDE_ROUTER_MODEL` (default
  `claude-haiku-4-5`) picks the project(s) from the catalog of `CLAUDE.md`
  headers.
- The session starts in the project root, so its `CLAUDE.md` is
  auto-discovered — no need to point at it explicitly.
- Remote sessions run inside `tmux new -A`, so if the connection drops,
  re-running the same `horde` command reattaches instead of starting over.
- Everything after `--` is passed through to `claude`.

### Host selection

If `HORDE_REMOTE` (or `programs.horde.client.remote`) is set, each
invocation probes the remote: reachable over SSH (BatchMode, 2s timeout) and
round-trip under `HORDE_LATENCY_MS` (default 150) → run remote; otherwise
run local.  `--local` / `--remote` force the choice, and `--host` overrides
the destination for one invocation.

Note: if you use SSH `ControlMaster` multiplexing, a warm connection makes
the latency probe read near-zero, biasing toward remote.

### Environment variables

The router (`horde`) reads `HORDE_REMOTE`, `HORDE_LATENCY_MS` (default 150),
`HORDE_CONNECT_TIMEOUT` (default 2), and `HORDE_ROUTER_MODEL` (default
`claude-haiku-4-5`).  The runner (`horde-run`) reads `HORDE_PROJECTS`,
`HORDE_STATE_DIR`, the token-file and expose-path variables, and
`HORDE_CLAUDE_SETTINGS`.  The home-manager module sets all of these from its
options; run `horde-run --help` for the full list if invoking it directly.

## Security model

`--dangerously-skip-permissions` removes Claude Code's own per-action
review, so isolation is the only perimeter — and horde makes that perimeter
the OS, not Claude's settings.

- **Outer bubblewrap namespace** is the real boundary: an explicit mount
  allowlist (project(s), `/nix/store`, private HOME, your `expose*` paths,
  minimal `/etc`).  Anything not listed — your home, `~/.ssh`, other
  projects, `/etc/shadow` — does not exist inside the sandbox, so even a
  fully compromised agent reading via any tool cannot reach it.  The
  environment is scrubbed to an allowlist, and secrets are passed via the
  environment (not argv).
- **Inner native sandbox** adds defense in depth, launched with:

  ```json
  {"sandbox": {"enabled": true, "failIfUnavailable": true, "allowUnsandboxedCommands": false}}
  ```

  `failIfUnavailable` turns a missing sandbox into a hard error rather than
  silent unsandboxed execution; `allowUnsandboxedCommands: false` disables
  the retry-outside-the-sandbox escape hatch.  Override with
  `programs.horde.runner.claudeSettings` (e.g. to add
  `sandbox.network.allowedDomains`).

Worth knowing:

- The boundary is still a shared kernel: it requires unprivileged user
  namespaces, and a kernel exploit escapes it.  For untrusted *inputs*
  (reviewing strangers' code, fetching arbitrary deps), step up to a VM —
  e.g. run the worker under `microvm.nix`; horde's design is unchanged, only
  what's underneath it.
- Egress is the residual exfiltration channel: even with an allowlist, an
  allowed domain like `github.com` is itself a usable channel, so prefer
  fine-grained, repo-scoped tokens.
- The optional seccomp filter that blocks Unix-domain sockets is not wired
  in (it ships as a global npm package, which fights NixOS); reachable
  sockets you expose (e.g. `allowNix`'s nix daemon) are outside the proxy.
- To stop a stray bare-host `claude --dangerously-skip-permissions` outside
  horde, set `permissions.disableBypassPermissionsMode = "disable"` in
  managed settings — horde-run's perimeter is unaffected.

## Syncthing caveats

- Make sure `.git` is **not** in your `.stignore`, or remote git operations
  will break.  Excluding `node_modules`/build artifacts is fine.
- Syncthing is eventually consistent: let a project settle before launching
  remotely, and treat the remote run as the sole writer — editing the same
  project locally mid-run produces `.sync-conflict` files.

## Development

`nix develop` (or direnv) drops you into a shell with a `cli` command:
`cli test nix lint|dead-code|format`, `cli test scripts`, `cli fix nix …`.
