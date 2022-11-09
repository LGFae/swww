{
  description = "A Solution to your Wayland Wallpaper Woes";

  inputs = { nixpkgs.url = "github:nixos/nixpkgs"; };

  outputs = { self, nixpkgs }:
    let pkgs = nixpkgs.legacyPackages.x86_64-linux;
    in {
      packages.x86_64-linux.swww = pkgs.rustPlatform.buildRustPackage rec {
        pname = "swww";
        version = "5.0.0";
        src = ./.;

        cargoLock.lockFile = ./Cargo.lock;
        buildType = "release";
        doCheck = false; # Fails to connect to socket during build

        nativeBuildInputs = with pkgs; [ pkg-config ];

        buildInputs = with pkgs; [ libxkbcommon lz4 ];
      };
      devShells.default = pkgs.mkShell {
        buildInputs = with pkgs; [ pkg-config libxkbcommon lz4 ];
      };
    };
}
