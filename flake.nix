{
  description = "swww, A Solution to your Wayland Wallpaper Woes";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    ...
  }:
    {
      overlays.default = final: prev: {inherit (self.packages.${prev.system}) swww;};
    }
    // flake-utils.lib.eachDefaultSystem (
      system: let
        inherit (nixpkgs) lib;

        pkgs = import nixpkgs {
          inherit system;
          overlays = [(import rust-overlay)];
        };

        cargo-toml = lib.importTOML ./Cargo.toml;
        inherit (cargo-toml.workspace.package) rust-version;
        rust = pkgs.rust-bin.stable.${rust-version}.default;

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rust;
          rustc = rust;
        };
      in {
        packages = {
          swww = rustPlatform.buildRustPackage {
            pname = "swww";

            src = pkgs.nix-gitignore.gitignoreSource [] ./.;
            inherit (cargo-toml.workspace.package) version;

            cargoLock.lockFile = ./Cargo.lock;

            buildInputs = with pkgs; [
              lz4
              libxkbcommon
              wayland-scanner
              wayland-protocols
            ];

            doCheck = false; # Integration tests do not work in sandbox environment

            nativeBuildInputs = with pkgs; [
              pkg-config
              installShellFiles
              scdoc
            ];

            postInstall = ''
              for f in doc/*.scd; do
                local page="doc/$(basename "$f" .scd)"
                scdoc < "$f" > "$page"
                installManPage "$page"
              done

              installShellCompletion --cmd swww \
                --bash completions/swww.bash \
                --fish completions/swww.fish \
                --zsh completions/_swww
            '';

            meta = {
              description = "Efficient animated wallpaper daemon for wayland, controlled at runtime";
              license = lib.licenses.gpl3;
              platforms = lib.platforms.linux;
              mainProgram = "swww";
            };
          };

          default = self.packages.${system}.swww;
        };

        formatter = pkgs.alejandra;

        devShells.default = pkgs.mkShell {
          inputsFrom = [self.packages.${system}.swww];

          packages = [pkgs.rust-bin.stable.${rust-version}.default];
        };
      }
    );
}
