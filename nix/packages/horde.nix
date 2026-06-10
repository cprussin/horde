{
  claude-code,
  coreutils,
  horde-run,
  jq,
  openssh,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde";
  runtimeInputs = [claude-code coreutils horde-run jq openssh];
  text = builtins.readFile ../../scripts/horde.sh;
}
