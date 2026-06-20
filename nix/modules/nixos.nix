{
  config,
  lib,
  ...
}: let
  cfg = config.programs.horde;
in {
  options.programs.horde.enable = lib.mkEnableOption ''
    the system-level requirement for running horde sandboxes on this machine:
    the unprivileged user-namespaces sysctl that bubblewrap needs.  Enable it
    on any machine that runs sessions, whether it dispatches them (client) or
    accepts them over SSH (server).  The packages and all other configuration
    live in the home-manager module
  '';

  config = lib.mkIf cfg.enable {
    # Both bubblewrap layers (horde-runner's outer namespace and Claude Code's
    # native sandbox nested inside it) need unprivileged user namespaces.
    # This is a kernel sysctl, so it can only be set system-wide.
    security.unprivilegedUsernsClone = lib.mkDefault true;
  };
}
