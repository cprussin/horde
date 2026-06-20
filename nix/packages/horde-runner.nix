{
  lib,
  rustPlatform,
  makeWrapper,
  bashInteractive,
  bubblewrap,
  claude-code,
  coreutils,
  gh,
  git,
  horde-gh-app-credential,
  ...
}:
rustPlatform.buildRustPackage {
  pname = "horde-runner";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../packages
    ];
  };

  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = ["--bin" "horde-runner"];
  doCheck = false;

  nativeBuildInputs = [makeWrapper];

  # bubblewrap builds the sandbox; claude-code runs inside it; git/gh/bash are
  # referenced by the generated credential helpers and gh wrapper;
  # horde-gh-app-credential is the App credential helper.  These also ride the
  # sandbox PATH so a session is functional without the module's sandbox-env.
  postInstall = ''
    wrapProgram $out/bin/horde-runner \
      --prefix PATH : ${lib.makeBinPath [
      bashInteractive
      bubblewrap
      claude-code
      coreutils
      gh
      git
      horde-gh-app-credential
    ]}
  '';

  meta = {
    description = "Sandboxed Claude Code launcher and PTY-streaming session service for horde";
    mainProgram = "horde-runner";
  };
}
