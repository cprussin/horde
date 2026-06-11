{
  bashInteractive,
  bubblewrap,
  claude-code,
  coreutils,
  gh,
  git,
  gnused,
  jq,
  socat,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde-run";
  # claude-code, bash, git, and gh ride along so the sandbox is functional
  # even without the NixOS module's sandbox-env PATH (everything resolves
  # from /nix/store, which is bound read-only inside).  gnused is used by
  # the environment scrub.
  runtimeInputs = [bashInteractive bubblewrap claude-code coreutils gh git gnused jq socat];
  text = builtins.readFile ../../scripts/horde-run.sh;
}
