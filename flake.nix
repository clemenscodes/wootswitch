{
  inputs = {
    nixpkgs = {
      url = "github:NixOS/nixpkgs/nixos-unstable";
    };
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs = {
          follows = "nixpkgs";
        };
      };
    };
  };

  outputs = {
    self,
    nixpkgs,
    flake-parts,
    crane,
    rust-overlay,
    ...
  } @ inputs: let
    system = "x86_64-linux";

    pkgs = import nixpkgs {
      inherit system;
      overlays = [(import rust-overlay)];
    };

    rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

    craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

    wootswitch = pkgs.callPackage ./nix/package.nix {inherit craneLib;};

    inherit (wootswitch.passthru) src commonArgs cargoArtifacts;
  in
    flake-parts.lib.mkFlake {inherit inputs;} {
      systems = [system];

      flake = {
        nixosModules = rec {
          wootswitch = import ./nix/module.nix self;
          default = wootswitch;
        };
      };

      perSystem = {
        config,
        system,
        ...
      }: {
        formatter = pkgs.alejandra;

        checks = {
          fmt = craneLib.cargoFmt {inherit src;};

          clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- --deny warnings";
            }
          );
        };

        packages = {
          default = wootswitch;
          inherit wootswitch;
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          buildInputs = with pkgs; [udev];
          nativeBuildInputs = with pkgs; [
            pkg-config
            rust-analyzer
            cargo-watch
            usbutils # lsusb — inspect HID devices
            hidapi # hidtest utility
          ];
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          RUST_BACKTRACE = 1;
        };
      };
    };

  nixConfig = {
    extra-substituters = [
      "https://nix-community.cachix.org"
      "https://clemenscodes.cachix.org"
    ];
    extra-trusted-public-keys = [
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
      "clemenscodes.cachix.org-1:yEwW1YgttL2xdsyfFDz/vv8zZRhRGMeDQsKKmtV1N18="
    ];
  };
}
