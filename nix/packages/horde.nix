{
  claude-code,
  coreutils,
  findutils,
  gnugrep,
  horde-run,
  jq,
  openssh,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde";
  # All external tools resolve from /nix/store via runtimeInputs, so horde
  # works without anything installed in the system environment: findutils
  # (find) and gnugrep (grep) round out coreutils for the project catalog
  # and router-output parsing.
  runtimeInputs = [claude-code coreutils findutils gnugrep horde-run jq openssh];
  text = builtins.readFile ../../scripts/horde.sh;
}
