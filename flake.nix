{
  description = "swww, A Solution to your Wayland Wallpaper Woes";

  # Nixpkgs / NixOS version to use.
  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  inputs.utils.url = "github:numtide/flake-utils";
  inputs.flake-compat = {
    url = "github:edolstra/flake-compat";
    flake = false;
  };

  outputs = {
    self,
    nixpkgs,
    utils,
    ...
  }:
    {
      overlays.default = final: prev: {
        swww = final.callPackage ./build.nix {};
      };
    }
    // utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [self.overlays.default];
      };
    in {
      packages = {
        inherit (pkgs) swww;
        default = pkgs.swww;
      };

      formatter = pkgs.alejandra;

      devShells.default = pkgs.callPackage ({
        mkShell,
        rustc,
        cargo,
        gnumake,
        pkg-config,
        lz4,
        libxkbcommon,
        wayland,
        wayland-protocols,
        swww,
      }:
        mkShell {
          inherit (swww) nativeBuildInputs buildInputs;
        }) {};
    });
}
