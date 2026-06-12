{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
    mkCli.url = "github:cprussin/mkCli";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    mkCli,
    ...
  }: let
    dev-cli = {
      lib,
      git,
      direnv,
      statix,
      deadnix,
      alejandra,
      shellcheck,
      ...
    }:
      lib.mkCli "cli" {
        _noAll = true;

        clean = "${lib.getExe git} clean -fdx && ${lib.getExe direnv} reload";

        test = {
          nix = {
            lint = "${lib.getExe statix} check --ignore node_modules .";
            dead-code = "${lib.getExe deadnix} --exclude ./node_modules ./third_party -- .";
            format = "${lib.getExe alejandra} --exclude ./node_modules --exclude ./third_party --check .";
          };
          scripts = "${lib.getExe shellcheck} --shell bash ./scripts/*.sh";
        };

        fix = {
          nix = {
            lint = "${lib.getExe statix} fix --ignore node_modules .";
            dead-code = "${lib.getExe deadnix} -e --exclude ./node_modules ./third_party -- .";
            format = "${lib.getExe alejandra} --exclude ./node_modules --exclude ./third_party .";
          };
        };
      };

    dev-shell = {
      mkShell,
      dev-cli,
      git,
      gh,
      claude-code,
      nodejs,
      ...
    }:
      mkShell {
        FORCE_COLOR = 1;
        name = "dev-shell";
        buildInputs = [
          dev-cli
          git
          gh
          nodejs
          claude-code
        ];
      };

    overlays = let
      mkOverlay = pkg-name: pkg: composedOverlays:
        nixpkgs.lib.composeManyExtensions (composedOverlays
          ++ [
            (final: prev: {"${pkg-name}" = final.callPackage pkg {inherit prev;};})
          ]);
    in {
      dev-cli = mkOverlay "dev-cli" dev-cli [mkCli.overlays.default];
      dev-shell = mkOverlay "dev-shell" dev-shell [overlays.dev-cli];
      horde-gh-app-credential = mkOverlay "horde-gh-app-credential" ./nix/packages/horde-gh-app-credential.nix [];
      horde-run = mkOverlay "horde-run" ./nix/packages/horde-run.nix [overlays.horde-gh-app-credential];
      horde = mkOverlay "horde" ./nix/packages/horde.nix [overlays.horde-run];
    };
  in
    (flake-utils.lib.eachDefaultSystem
      (
        system: let
          pkg-from-overlay = overlay-name:
            (import nixpkgs {
              inherit system;
              overlays = [overlays."${overlay-name}"];
              config.allowUnfreePredicate = pkg: builtins.elem (nixpkgs.lib.getName pkg) ["claude-code"];
            })."${overlay-name}";
        in {
          packages = nixpkgs.lib.mapAttrs (name: _: pkg-from-overlay name) overlays;
          devShells.default = pkg-from-overlay "dev-shell";
        }
      ))
    // {
      inherit overlays;

      nixosModules.default = ./nix/modules/nixos.nix;

      homeManagerModules.default = {
        lib,
        pkgs,
        ...
      }: {
        imports = [./nix/modules/home-manager.nix];
        programs.horde.client.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.horde;
        programs.horde.runner.package =
          lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.horde-run;
      };

      # home-manager renamed homeManagerModules to homeModules; expose both.
      homeModules = self.homeManagerModules;
    };
}
