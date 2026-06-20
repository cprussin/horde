{
  bashInteractive,
  bubblewrap,
  claude-code,
  coreutils,
  gh,
  git,
  gnugrep,
  gnused,
  horde-gh-app-credential,
  jq,
  socat,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde-run";
  # claude-code, bash, git, and gh ride along so the sandbox is functional
  # even without the NixOS module's sandbox-env PATH (everything resolves
  # from /nix/store, which is bound read-only inside).  gnused and gnugrep
  # are used by the environment scrub and the gitconfig include check.
  # horde-gh-app-credential is referenced (by store path) as a git
  # credential helper when a GitHub App is configured.
  runtimeInputs = [bashInteractive bubblewrap claude-code coreutils gh git gnugrep gnused horde-gh-app-credential jq socat];
  text = builtins.readFile ../../scripts/horde-run.sh;
}
