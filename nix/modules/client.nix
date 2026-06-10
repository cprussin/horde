{
  config,
  lib,
  ...
}: let
  cfg = config.programs.horde.client;

  env-var = name: value:
    lib.optionalAttrs (value != null) {"${name}" = toString value;};
in {
  imports = [./runner.nix];

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

  config = lib.mkIf cfg.enable {
    # Local runs go through the same sandboxed runner as remote ones.
    programs.horde.runner.enable = true;

    environment.systemPackages = [cfg.package];

    environment.variables =
      env-var "HORDE_REMOTE" cfg.remote
      // env-var "HORDE_ROUTER_MODEL" cfg.routerModel
      // env-var "HORDE_LATENCY_MS" cfg.latencyMs
      // env-var "HORDE_CONNECT_TIMEOUT" cfg.connectTimeout;
  };
}
