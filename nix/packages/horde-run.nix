{
  bubblewrap,
  claude-code,
  coreutils,
  socat,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde-run";
  runtimeInputs = [bubblewrap claude-code coreutils socat];
  text = builtins.readFile ../../scripts/horde-run.sh;
}
