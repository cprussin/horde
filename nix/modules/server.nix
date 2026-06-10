{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.horde.server;
in {
  imports = [./runner.nix];

  options.programs.horde.server = {
    enable = lib.mkEnableOption "the horde server (remote sandboxed Claude Code runner)";
  };

  config = lib.mkIf cfg.enable {
    programs.horde.runner.enable = true;

    # tmux is invoked directly by the client's remote handoff so a dropped
    # SSH connection doesn't kill the session.
    environment.systemPackages = [pkgs.tmux];

    services.openssh.enable = lib.mkDefault true;
  };
}
