{
  config,
  lib,
  ...
}: let
  cfg = config.programs.horde;
  active = cfg.client.enable || cfg.server.enable;
in {
  options.programs.horde = {
    client.enable = lib.mkEnableOption ''
      the system-level requirements for running horde sessions locally
      (the unprivileged user-namespaces sysctl that bubblewrap needs).  The
      client package and its configuration live in the home-manager module
    '';

    server.enable = lib.mkEnableOption ''
      the system-level requirements for accepting remote horde sessions
      (the user-namespaces sysctl plus sshd).  The runner package and its
      configuration live in the home-manager module
    '';
  };

  config = lib.mkIf active {
    # Both bubblewrap layers (horde-run's outer namespace and Claude Code's
    # native sandbox nested inside it) need unprivileged user namespaces.
    # This is a kernel sysctl, so it can only be set system-wide.
    security.unprivilegedUsernsClone = lib.mkDefault true;

    # The remote host must accept SSH for the client's handoff to reach it.
    services.openssh.enable = lib.mkIf cfg.server.enable (lib.mkDefault true);
  };
}
