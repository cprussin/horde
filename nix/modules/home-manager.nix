{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.horde;

  # The runner (sandboxed session launcher) is active on any machine that
  # runs sessions: the client runs them locally, the server runs them for
  # remote handoffs.
  runner-active = cfg.client.enable || cfg.server.enable;

  sandbox-env = pkgs.buildEnv {
    name = "horde-sandbox-env";
    # nix is only useful inside the sandbox when the daemon socket is
    # exposed (allowNix), so it rides along only then.
    paths = cfg.runner.packages ++ lib.optional cfg.runner.allowNix pkgs.nix;
  };

  env-var = name: value:
    lib.optionalAttrs (value != null) {"${name}" = toString value;};
in {
  options.programs.horde = {
    client = {
      enable = lib.mkEnableOption "the horde client (project router and dispatcher) for this user";

      package = lib.mkOption {
        type = lib.types.package;
        description = "The horde client package.";
      };

      remote = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "me@server.lan";
        description = ''
          SSH destination of the remote execution host.  When unset, all
          sessions run locally.  New sessions are placed here (or local) by the
          latency gate; the session switcher also discovers sessions here.
        '';
      };

      remotes = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        example = ["me@build-a.lan" "me@build-b.lan"];
        description = ''
          Additional SSH destinations the session switcher discovers running
          sessions on (in addition to `remote` and local).  Display/recovery
          only — new sessions are still placed on `remote` (or local).
        '';
      };

      routerModel = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "claude-haiku-4-5";
        description = "Model used for the headless project-routing call.";
      };

      latencyMs = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        example = 150;
        description = ''
          Maximum SSH round-trip latency in milliseconds before execution
          falls back to local.
        '';
      };

      connectTimeout = lib.mkOption {
        type = lib.types.nullOr lib.types.int;
        default = null;
        example = 2;
        description = "SSH reachability probe timeout in seconds.";
      };
    };

    server = {
      enable = lib.mkEnableOption "the horde server (remote sandboxed Claude Code runner) for this user";
    };

    runner = {
      package = lib.mkOption {
        type = lib.types.package;
        description = "The package providing horde-runner (the sandboxed launcher and session service).";
      };

      projectsDir = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/home/alice/Projects";
        description = ''
          Directory containing the project subdirectories.  Defaults to
          ~/Projects.  Only the project(s) selected for a session are
          exposed inside its sandbox, not the whole directory.
        '';
      };

      stateDir = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          Host directory bound as the sandbox HOME, persisting Claude Code
          state (session history, project trust, git credential config)
          across sessions.  Defaults to ~/.local/share/horde/home.
        '';
      };

      claudeTokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/secrets/claude-token";
        description = ''
          File containing a Claude credential, exported inside the sandbox
          as CLAUDE_CODE_OAUTH_TOKEN (if it starts with sk-ant-oat, as
          produced by `claude setup-token`) or ANTHROPIC_API_KEY otherwise.
          Keep it out of the nix store — deploy with sops-nix/agenix and
          point this at the decrypted path.
        '';
      };

      githubTokenFiles = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = {};
        example = {
          default = "/run/secrets/github-default";
          acme-corp = "/run/secrets/github-acme";
          side-org = "/run/secrets/github-side";
        };
        description = ''
          GitHub tokens, keyed by owner (organization or user login).  Each
          token is bound to that owner's repos (https://github.com/<owner>)
          via a generated git credential config, so the agent automatically
          uses the right token per repo — letting you scope a separate
          fine-grained PAT to each organization.  The special "default" key
          is the host-level fallback for any other owner and is also exported
          as GH_TOKEN/GITHUB_TOKEN.  When more than one owner is configured, a
          gh wrapper selects the matching token by the current repo's owner.
          Use HTTPS remotes inside the sandbox (git-over-SSH does not pass
          through the native sandbox's proxy).  Keep the files out of the nix
          store — deploy them with sops-nix/agenix.
        '';
      };

      githubApp = {
        appId = lib.mkOption {
          type = lib.types.nullOr (lib.types.either lib.types.int lib.types.str);
          default = null;
          example = 123456;
          description = "GitHub App ID (from the App's settings page).";
        };

        privateKeyFile = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "/run/secrets/github-app-key.pem";
          description = ''
            File containing the GitHub App private key (PEM).  Keep it out of
            the nix store — deploy with sops-nix/agenix.
          '';
        };
      };

      extraTokenFiles = lib.mkOption {
        type = lib.types.attrsOf lib.types.str;
        default = {};
        example = {
          CACHIX_AUTH_TOKEN = "/run/secrets/cachix-token";
        };
        description = ''
          Additional secrets for other services: environment variable name
          to token file path.  Each file's contents are exported inside the
          sandbox under the given variable name.
        '';
      };

      exposeReadOnly = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        example = ["/home/alice/reference-docs"];
        description = ''
          Extra host paths exposed read-only inside the sandbox, at the same
          path.
        '';
      };

      exposeReadWrite = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        example = ["/var/cache/horde-builds"];
        description = ''
          Extra host paths exposed read-write inside the sandbox, at the
          same path.
        '';
      };

      packages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = with pkgs; [
          bashInteractive
          coreutils
          curl
          diffutils
          findutils
          gawk
          gh
          git
          gnugrep
          gnused
          gnutar
          gzip
          jq
          ripgrep
        ];
        description = ''
          Packages available on PATH inside the sandbox (claude itself is
          carried by the horde-runner package).  These come from /nix/store,
          which is exposed read-only.
        '';
      };

      allowNix = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Let sessions use nix inside the sandbox — needed for `nix develop`
          / direnv, including on worktrees created mid-session.  Exposes the
          nix daemon socket and /etc/nix, adds nix to the sandbox PATH, and
          forces the daemon store (NIX_REMOTE=daemon) so builds and
          substitutions go to the real store rather than a private chroot
          store.  This lets the agent realize arbitrary store paths and
          consume build resources via the daemon, so it is off by default.
        '';
      };

      claudeSettings = lib.mkOption {
        type = lib.types.nullOr (lib.types.attrsOf lib.types.anything);
        default = null;
        example = {
          sandbox = {
            enabled = true;
            failIfUnavailable = true;
            allowUnsandboxedCommands = false;
            network.allowedDomains = ["github.com" "registry.npmjs.org"];
          };
        };
        description = ''
          Settings passed to claude via --settings, replacing horde-runner's
          built-in strict inner-sandbox settings.
        '';
      };
    };
  };

  config = lib.mkIf runner-active {
    assertions = [
      {
        assertion =
          (cfg.runner.githubApp.appId == null) == (cfg.runner.githubApp.privateKeyFile == null);
        message = "programs.horde.runner.githubApp needs both appId and privateKeyFile, or neither.";
      }
    ];

    home.packages =
      lib.optional cfg.client.enable cfg.client.package
      ++ lib.optional cfg.server.enable cfg.runner.package;

    home.sessionVariables =
      # Runner variables are needed wherever sessions run: on the client for
      # local runs, on the server for remote ones.
      {HORDE_SANDBOX_PATH = "${sandbox-env}/bin";}
      // env-var "HORDE_PROJECTS" cfg.runner.projectsDir
      // env-var "HORDE_STATE_DIR" cfg.runner.stateDir
      // env-var "HORDE_CLAUDE_TOKEN_FILE" cfg.runner.claudeTokenFile
      // lib.optionalAttrs (cfg.runner.githubTokenFiles != {}) {
        HORDE_GITHUB_TOKEN_FILES = builtins.toJSON cfg.runner.githubTokenFiles;
      }
      // lib.optionalAttrs (cfg.runner.githubApp.appId != null) {
        HORDE_GH_APP_ID = toString cfg.runner.githubApp.appId;
        HORDE_GH_APP_KEY_FILE = cfg.runner.githubApp.privateKeyFile;
      }
      // lib.optionalAttrs (cfg.runner.extraTokenFiles != {}) {
        HORDE_TOKEN_FILES = builtins.toJSON cfg.runner.extraTokenFiles;
      }
      // lib.optionalAttrs (cfg.runner.exposeReadOnly != []) {
        HORDE_RO_PATHS = builtins.toJSON cfg.runner.exposeReadOnly;
      }
      // lib.optionalAttrs (cfg.runner.exposeReadWrite != []) {
        HORDE_RW_PATHS = builtins.toJSON cfg.runner.exposeReadWrite;
      }
      // lib.optionalAttrs cfg.runner.allowNix {HORDE_ALLOW_NIX = "1";}
      // lib.optionalAttrs (cfg.runner.claudeSettings != null) {
        HORDE_CLAUDE_SETTINGS = builtins.toJSON cfg.runner.claudeSettings;
      }
      # Client-only variables: the router and host gate.
      // lib.optionalAttrs cfg.client.enable (
        env-var "HORDE_REMOTE" cfg.client.remote
        // env-var "HORDE_ROUTER_MODEL" cfg.client.routerModel
        // env-var "HORDE_LATENCY_MS" cfg.client.latencyMs
        // env-var "HORDE_CONNECT_TIMEOUT" cfg.client.connectTimeout
        // lib.optionalAttrs (cfg.client.remotes != []) {
          HORDE_REMOTES = lib.concatStringsSep " " cfg.client.remotes;
        }
      );
  };
}
