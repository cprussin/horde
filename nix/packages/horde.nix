{
  lib,
  rustPlatform,
  makeWrapper,
  claude-code,
  horde-runner,
  openssh,
  ...
}:
rustPlatform.buildRustPackage {
  pname = "horde";
  version = "0.1.0";

  # The whole workspace must be present for cargo to resolve members, even
  # though we only build the `horde` binary.
  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../packages
    ];
  };

  cargoLock.lockFile = ../../Cargo.lock;
  cargoBuildFlags = ["--bin" "horde"];
  # Tests run via `cli test` in the dev shell, not at package-build time.
  doCheck = false;

  nativeBuildInputs = [makeWrapper];

  # The client execs `horde-runner` for local runs and `ssh` for remote ones;
  # `claude` is the routing call.  Prefix so the remote login PATH still wins.
  postInstall = ''
    wrapProgram $out/bin/horde \
      --prefix PATH : ${lib.makeBinPath [claude-code openssh horde-runner]}
  '';

  meta = {
    description = "Route a prompt to the right project and run Claude Code on it, sandboxed";
    mainProgram = "horde";
  };
}
