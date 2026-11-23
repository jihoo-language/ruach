{
  description = "Ruach: a functional general-purpose language that compiles to C";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";

  outputs = { nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      package = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "ruach";
        version = "0.1.0";
        src = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            ./Cargo.lock
            ./Cargo.toml
            ./README.md
            ./examples
            ./src
            ./tests
          ];
        };
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.makeWrapper ];
        # The compiler writes C; the installed binary knows which C compiler to use.
        postInstall = ''
          wrapProgram $out/bin/ruach --set-default RUACH_CC ${pkgs.stdenv.cc}/bin/cc
        '';
        meta = {
          description = "Compiler for Ruach";
          license = pkgs.lib.licenses.asl20;
          mainProgram = "ruach";
        };
      };
    in
    {
      packages = forSystems (pkgs: { default = package pkgs; });
      apps = forSystems (pkgs: {
        default = {
          type = "app";
          program = pkgs.lib.getExe (package pkgs);
          meta.description = "Compile and run a Ruach program";
        };
      });
      devShells = forSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy stdenv.cc python3 ];
        };
      });
      checks = forSystems (pkgs: { default = package pkgs; });
    };
}
