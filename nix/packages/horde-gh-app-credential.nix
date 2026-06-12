{
  coreutils,
  curl,
  gnused,
  jq,
  openssl,
  writeShellApplication,
  ...
}:
writeShellApplication {
  name = "horde-gh-app-credential";
  runtimeInputs = [coreutils curl gnused jq openssl];
  text = builtins.readFile ../../scripts/horde-gh-app-credential.sh;
}
