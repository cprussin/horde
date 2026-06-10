{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.horde.server;

  env-var = name: value:
    lib.optionalAttrs (value != null) {"${name}" = toString value;};
in {
  options.programs.horde.server = {
    enable = lib.mkEnableOption "the horde server (remote sandboxed Claude Code runner)";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The package providing horde-run.";
    };

    projectsDir = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/home/alice/Projects";
      description = ''
        Directory containing the project subdirectories (kept in sync with
        the client, e.g. via Syncthing).  Defaults to ~/Projects of the
        invoking user.
      '';
    };

    apiKeyFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/run/secrets/anthropic-api-key";
      description = ''
        File whose contents are exported as ANTHROPIC_API_KEY by horde-run.
        Keep it out of the nix store — deploy it with sops-nix or agenix
        and point this option at the decrypted path.
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
        built-in strict sandbox settings.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # tmux is invoked directly by the client's remote handoff so a dropped
    # SSH connection doesn't kill the session.
    environment.systemPackages = [cfg.package pkgs.tmux];

    environment.variables =
      env-var "HORDE_PROJECTS" cfg.projectsDir
      // env-var "HORDE_API_KEY_FILE" cfg.apiKeyFile
      // lib.optionalAttrs (cfg.claudeSettings != null) {
        HORDE_CLAUDE_SETTINGS = builtins.toJSON cfg.claudeSettings;
      };

    # Claude Code's Linux sandbox is built on bubblewrap, which needs
    # unprivileged user namespaces.
    security.unprivilegedUsernsClone = lib.mkDefault true;

    services.openssh.enable = lib.mkDefault true;
  };
}
