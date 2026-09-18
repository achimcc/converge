{
  description = "Reconcile self-hosted services with a desired state through their HTTP APIs";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  # The RustSec advisory database, pinned like any other input. The `audit`
  # check reads it offline; `nix flake update advisory-db` brings news in.
  inputs.advisory-db = {
    url = "github:rustsec/advisory-db";
    flake = false;
  };

  outputs =
    { self, nixpkgs, advisory-db }:
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
        # Known advisories against Cargo.lock, read offline from the pinned
        # database. RUSTSEC-2026-0285 (rustls) sat in two deployed binaries of
        # sibling projects for four days before an audit found it by hand.
        audit = pkgs.runCommand "converge-audit" { nativeBuildInputs = [ pkgs.cargo-audit ]; } ''
          HOME=$TMPDIR cargo-audit audit --no-fetch --db ${advisory-db} --file ${./Cargo.lock}
          touch $out
        '';
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
