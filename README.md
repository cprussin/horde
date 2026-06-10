# horde

Run Claude Code tasks against your projects from anywhere, sandboxed, on
whichever machine is best.

You type a prompt; horde figures out which project in `~/Projects` you mean
(using a cheap headless Claude routing call against each project's
`CLAUDE.md`), picks a host (local, or a remote server if it's reachable with
low latency), and launches `claude --dangerously-skip-permissions` in that
project — contained inside Claude Code's native OS sandbox (bubblewrap +
socat), which horde force-enables for every session.

Project files are assumed to already exist at the same path on both machines
(e.g. synced with Syncthing); there is no sync layer.

## Components

| Piece       | Runs on | Job                                                                  |
| ----------- | ------- | -------------------------------------------------------------------- |
| `horde`     | client  | Catalog projects, route the prompt, gate local/remote, hand off      |
| `horde-run` | both    | Enter the project, force-enable the sandbox, launch the session      |

The remote host never sees the router or any project-selection logic.  Its
entire footprint is `horde-run` (which carries `claude-code`, `bubblewrap`,
and `socat` with it), `tmux`, `sshd`, and the user-namespaces sysctl.

## Installation

Add the flake as an input and import the modules:

```nix
{
  inputs.horde.url = "github:cprussin/horde";

  # Local machine (the one you type on):
  nixosConfigurations.laptop.modules = [
    inputs.horde.nixosModules.client
    {
      programs.horde.client = {
        enable = true;
        remote = "me@server.lan";   # omit to always run locally
      };
    }
  ];

  # Remote execution host:
  nixosConfigurations.server.modules = [
    inputs.horde.nixosModules.server
    {
      programs.horde.server = {
        enable = true;
        apiKeyFile = "/run/secrets/anthropic-api-key";  # via sops-nix/agenix
      };
    }
  ];
}
```

`nixosModules.default` imports both, for a machine that plays both roles
(when both are enabled, the server module owns the shared runner settings).
The packages are also exposed directly as `packages.<system>.horde` and
`packages.<system>.horde-run`, and as overlays, if you'd rather wire things
up yourself.  The flake instantiates its own nixpkgs with the `claude-code`
unfree exception, so you don't need to touch your system's `allowUnfree`
settings.

### Authentication

- **Local**: log in to Claude Code normally (`claude` → `/login`); horde
  uses the same credentials.  Alternatively set
  `programs.horde.client.apiKeyFile`.
- **Remote**: set `programs.horde.server.apiKeyFile` to a path containing an
  API key.  Keep the key out of the nix store — deploy it with sops-nix or
  agenix and point the option at the decrypted path (mode `0600`).

### Third-party services (GitHub, etc.)

Sessions inherit the worker's file-based credentials: the sandbox allows
filesystem **reads** everywhere by default, so tokens written by a one-time
login (`~/.config/gh`, `~/.netrc`, `~/.git-credentials`, …) are visible to
every session with no further prompts.

One-time GitHub setup per worker:

```bash
gh auth login        # device flow, works over SSH; or paste a fine-grained PAT
gh auth setup-git    # makes gh the git credential helper for HTTPS remotes
```

(Have `gh`/`git` on the worker's PATH for sandbox sessions — via the
project's dev shell or `environment.systemPackages`.)

Caveats:

- **Use HTTPS remotes inside workers, not SSH.**  Sandboxed network traffic
  goes through a hostname-based HTTP(S) proxy; git-over-SSH (port 22)
  doesn't pass through it.  HTTPS+token is also the safer choice: a
  bypass-permissions agent can read anything readable, so prefer
  fine-grained, repo-scoped PATs over your real SSH keys.
- Writes outside the project are denied by default.  Reading stored tokens
  is unaffected, but if a service's CLI needs to update its own config
  mid-session, allow it via `claudeSettings`, e.g.
  `sandbox.filesystem.allowWrite = ["~/.config/gh"]`.
- If you lock egress with `sandbox.network.allowedDomains`, include each
  service's domains — GitHub needs `github.com`, `api.github.com`,
  `codeload.github.com`, and `objects.githubusercontent.com`.

The same pattern works for any service whose CLI caches a token in a file
after a one-time login.  Conversely, remember the flip side: every credential
file on the worker is readable by the agent, so keep secrets you *don't*
want exposed off the worker (or add them to `sandbox.filesystem.denyRead`).

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

| Variable                | Default            | Meaning                                  |
| ----------------------- | ------------------ | ---------------------------------------- |
| `HORDE_PROJECTS`        | `~/Projects`       | Directory containing projects            |
| `HORDE_REMOTE`          | _(unset = local)_  | SSH destination of the remote host       |
| `HORDE_LATENCY_MS`      | `150`              | Max RTT before falling back to local     |
| `HORDE_CONNECT_TIMEOUT` | `2`                | Reachability probe timeout (seconds)     |
| `HORDE_ROUTER_MODEL`    | `claude-haiku-4-5` | Model for the routing call               |
| `HORDE_API_KEY_FILE`    | _(unset)_          | File exported as `ANTHROPIC_API_KEY`     |
| `HORDE_CLAUDE_SETTINGS` | _(built-in)_       | JSON/path overriding the sandbox settings |

The NixOS modules set these system-wide from their options.

## Security model

`--dangerously-skip-permissions` removes per-action review, so the sandbox
is the only perimeter.  horde-run therefore passes strict settings on every
launch:

```json
{"sandbox": {"enabled": true, "failIfUnavailable": true, "allowUnsandboxedCommands": false}}
```

- `failIfUnavailable` makes a missing sandbox a hard error rather than a
  silent fallback to unsandboxed execution.
- `allowUnsandboxedCommands: false` disables the escape hatch that retries
  failed commands outside the sandbox.
- Filesystem writes are confined to the project (plus `--add-dir` extras);
  network egress goes through the sandbox proxy.  To lock egress to an
  allowlist, set `claudeSettings` (client and/or server module) with
  `sandbox.network.allowedDomains` — e.g. GitHub, npm, and the Claude API.
- The sandbox protects the host from the agent, not from the invoking user.

Worth knowing:

- An exfiltration-capable agent can still read anything inside the sandbox,
  so don't `--add-dir` directories containing secrets, and prefer
  short-lived repo-scoped tokens inside projects.
- To prevent a stray bare-host `claude --dangerously-skip-permissions`
  outside horde, set `permissions.disableBypassPermissionsMode = "disable"`
  in managed settings — horde-run's `--settings` perimeter is unaffected.

## Syncthing caveats

- Make sure `.git` is **not** in your `.stignore`, or remote git operations
  will break.  Excluding `node_modules`/build artifacts is fine.
- Syncthing is eventually consistent: let a project settle before launching
  remotely, and treat the remote run as the sole writer — editing the same
  project locally mid-run produces `.sync-conflict` files.

## Development

`nix develop` (or direnv) drops you into a shell with a `cli` command:
`cli test nix lint|dead-code|format`, `cli test scripts`, `cli fix nix …`.
