{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.horde.runner;

  sandbox-env = pkgs.buildEnv {
    name = "horde-sandbox-env";
    paths = cfg.packages;
  };

  env-var = name: value:
    lib.optionalAttrs (value != null) {"${name}" = toString value;};
in {
  options.programs.horde.runner = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      internal = true;
      description = ''
        Whether the horde runner environment is configured on this machine.
        Set by the client and server modules; not intended to be set
        directly.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      description = "The package providing horde-run.";
    };

    projectsDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/home/alice/Projects";
      description = ''
        Directory containing the project subdirectories.  Defaults to
        ~/Projects of the invoking user.  Only the project(s) selected for
        a session are exposed inside its sandbox, not the whole directory.
      '';
    };

    stateDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        Host directory bound as the sandbox HOME, persisting Claude Code
        state (session history, project trust, git credential config)
        across sessions.  Defaults to ~/.local/share/horde/home of the
        invoking user.
      '';
    };

    claudeTokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/claude-token";
      description = ''
        File containing a Claude credential, exported inside the sandbox as
        CLAUDE_CODE_OAUTH_TOKEN (if it starts with sk-ant-oat, as produced
        by `claude setup-token`) or ANTHROPIC_API_KEY otherwise.  Keep it
        out of the nix store — deploy with sops-nix/agenix and point this
        at the decrypted path.
      '';
    };

    githubTokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/github-token";
      description = ''
        File containing a GitHub token, exported inside the sandbox as
        GH_TOKEN and GITHUB_TOKEN; gh is also wired up as git's credential
        helper for github.com so HTTPS remotes work without further setup.
        Prefer a fine-grained PAT scoped to the repos agents work on.
        Keep it out of the nix store.
      '';
    };

    extraTokenFiles = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = {};
      example = {
        CACHIX_AUTH_TOKEN = "/run/secrets/cachix-token";
      };
      description = ''
        Additional secrets for other services: environment variable name to
        token file path.  Each file's contents are exported inside the
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
        Extra host paths exposed read-write inside the sandbox, at the same
        path.
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
        carried by the horde-run package).  These come from /nix/store,
        which is exposed read-only.
      '';
    };

    allowNix = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Expose the nix daemon socket inside the sandbox so sessions can run
        nix builds and dev shells.  This lets the agent realize arbitrary
        store paths and consume build resources, so it is off by default.
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
        Settings passed to claude via --settings, replacing horde-run's
        built-in strict inner-sandbox settings.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [cfg.package];

    environment.variables =
      {HORDE_SANDBOX_PATH = "${sandbox-env}/bin";}
      // env-var "HORDE_PROJECTS" cfg.projectsDir
      // env-var "HORDE_STATE_DIR" cfg.stateDir
      // env-var "HORDE_CLAUDE_TOKEN_FILE" cfg.claudeTokenFile
      // env-var "HORDE_GITHUB_TOKEN_FILE" cfg.githubTokenFile
      // lib.optionalAttrs (cfg.extraTokenFiles != {}) {
        HORDE_TOKEN_FILES = builtins.toJSON cfg.extraTokenFiles;
      }
      // lib.optionalAttrs (cfg.exposeReadOnly != []) {
        HORDE_RO_PATHS = builtins.toJSON cfg.exposeReadOnly;
      }
      // lib.optionalAttrs (cfg.exposeReadWrite != []) {
        HORDE_RW_PATHS = builtins.toJSON cfg.exposeReadWrite;
      }
      // lib.optionalAttrs cfg.allowNix {HORDE_ALLOW_NIX = "1";}
      // lib.optionalAttrs (cfg.claudeSettings != null) {
        HORDE_CLAUDE_SETTINGS = builtins.toJSON cfg.claudeSettings;
      };

    # Both bubblewrap layers (horde-run's outer namespace and Claude Code's
    # native sandbox nested inside it) need unprivileged user namespaces.
    security.unprivilegedUsernsClone = lib.mkDefault true;
  };
}
