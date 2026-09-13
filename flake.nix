{
  description = "Reconcile self-hosted services with a desired state through their HTTP APIs";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "converge";
          # Read out of Cargo.toml rather than written down a second time, so
          # the store path and the crate cannot disagree about the version.
          version = (nixpkgs.lib.importTOML ./Cargo.toml).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          meta = {
            description = "Reconcile self-hosted services with a desired state through their HTTP APIs";
            license = pkgs.lib.licenses.agpl3Only;
            mainProgram = "converge";
          };
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy ];
        };
      });

      checks = forAll (pkgs: {
        package = self.packages.${pkgs.system}.default;
        clippy = self.packages.${pkgs.system}.default.overrideAttrs (old: {
          pname = "converge-clippy";
          nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
          buildPhase = "cargo clippy --all-targets -- -D warnings";
          installPhase = "touch $out";
        });
        fmt = self.packages.${pkgs.system}.default.overrideAttrs (old: {
          pname = "converge-fmt";
          nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.rustfmt ];
          buildPhase = "cargo fmt --check";
          installPhase = "touch $out";
        });
      });
    };
}
