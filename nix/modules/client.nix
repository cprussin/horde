{
  config,
  lib,
  ...
}: let
  cfg = config.programs.horde.client;

  # The runner settings (HORDE_PROJECTS et al) are shared with the server
  # module; when both are enabled on one machine, the server module owns
  # those variables so the two never define them twice.
  server-enabled = config.programs.horde.server.enable or false;

  env-var = name: value:
    lib.optionalAttrs (value != null) {"${name}" = toString value;};
in {
  options.programs.horde.client = {
    enable = lib.mkEnableOption "the horde client (project router and dispatcher)";

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
        sessions run locally.
      '';
    };

    projectsDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/home/alice/Projects";
      description = ''
        Directory containing the project subdirectories.  Defaults to
        ~/Projects of the invoking user.
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

    apiKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = ''
        File whose contents are exported as ANTHROPIC_API_KEY for local
        runs.  Keep it out of the nix store (e.g. deploy with sops-nix or
        agenix).  Unnecessary if you log in to Claude Code normally on this
        machine.
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
        Settings passed to claude via --settings for local sandboxed runs,
        replacing horde-run's built-in strict sandbox settings.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [cfg.package];

    environment.variables =
      env-var "HORDE_REMOTE" cfg.remote
      // env-var "HORDE_ROUTER_MODEL" cfg.routerModel
      // env-var "HORDE_LATENCY_MS" cfg.latencyMs
      // env-var "HORDE_CONNECT_TIMEOUT" cfg.connectTimeout
      // lib.optionalAttrs (!server-enabled) (
        env-var "HORDE_PROJECTS" cfg.projectsDir
        // env-var "HORDE_API_KEY_FILE" cfg.apiKeyFile
        // lib.optionalAttrs (cfg.claudeSettings != null) {
          HORDE_CLAUDE_SETTINGS = builtins.toJSON cfg.claudeSettings;
        }
      );

    # Claude Code's Linux sandbox is built on bubblewrap, which needs
    # unprivileged user namespaces.
    security.unprivilegedUsernsClone = lib.mkDefault true;
  };
}
