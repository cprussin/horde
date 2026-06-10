{
  bashInteractive,
  bubblewrap,
  claude-code,
  coreutils,
  gh,
  git,
  jq,
  socat,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde-run";
  # claude-code, bash, git, and gh ride along so the sandbox is functional
  # even without the NixOS module's sandbox-env PATH (everything resolves
  # from /nix/store, which is bound read-only inside).
  runtimeInputs = [bashInteractive bubblewrap claude-code coreutils gh git jq socat];
  text = builtins.readFile ../../scripts/horde-run.sh;
}
