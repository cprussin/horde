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
and `socat` with it), `tmux`, `sshd`, and the user-namespaces sysctl.

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

- a single **NixOS module** (`nixosModules.default`) for the only
  system-level needs — the unprivileged user-namespaces sysctl bubblewrap
  depends on, and (for a server) sshd;
- a **home-manager module** (`homeManagerModules.default`, also exported as
  `homeModules.default`) for everything user-space — the packages and all
  configuration.

Enable a role with `programs.horde.client.enable` / `.server.enable` in the
NixOS module, and configure that role under the same option names in
home-manager.

```nix
# NixOS config — system-level requirements only:
{
  imports = [ inputs.horde.nixosModules.default ];
  programs.horde.client.enable = true;   # laptop; use .server.enable on the server
}
```

```nix
# home-manager config — packages, router, sandbox, and secrets:
{
  imports = [ inputs.horde.homeManagerModules.default ];

  programs.horde.client = {
    enable = true;
    remote = "me@server.lan";            # omit to always run locally
  };
  programs.horde.runner = {
    # Secrets, deployed out-of-store via sops-nix/agenix:
    claudeTokenFile = "/run/secrets/claude-token";
    githubTokenFile = "/run/secrets/github-token";
  };
}
```

On the server, set `programs.horde.server.enable = true` in both modules
instead, and give the home-manager `programs.horde.runner` the same secret
files.  A machine that plays both roles enables both.

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
| `githubTokenFile` | File with a GitHub token; also wires gh as git's credential helper      |
| `extraTokenFiles` | `{ VAR = "/path"; }` — other secrets, exported under the given names     |
| `exposeReadOnly`  | Extra host paths mounted read-only in the sandbox                       |
| `exposeReadWrite` | Extra host paths mounted read-write in the sandbox                      |
| `packages`        | Tools available on PATH inside the sandbox (sensible default set)        |
| `allowNix`        | Expose the nix daemon socket so sessions can build (default `false`)     |
| `claudeSettings`  | Override the inner native-sandbox `--settings` (e.g. egress allowlist)   |

### Authentication

Provide credentials as files (deploy them out of the nix store with
sops-nix/agenix, mode `0600`); nothing needs an interactive login per
session.

- **Claude** — `claudeTokenFile`.  Run `claude setup-token` once to mint a
  long-lived OAuth token (starts with `sk-ant-oat`); horde exports it as
  `CLAUDE_CODE_OAUTH_TOKEN`.  Any other value is treated as an API key and
  exported as `ANTHROPIC_API_KEY`.
- **GitHub** — `githubTokenFile`, ideally a fine-grained, repo-scoped PAT.
  horde exports it as `GH_TOKEN`/`GITHUB_TOKEN` and configures gh as git's
  HTTPS credential helper in the sandbox HOME, so `git push`, `gh pr
  create`, etc. work with no per-session auth.
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
