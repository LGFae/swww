{
  description = "A Solution to your Wayland Wallpaper Woes";

  inputs = { nixpkgs.url = "github:nixos/nixpkgs"; };

  outputs = { self, nixpkgs }:
    let pkgs = nixpkgs.legacyPackages.x86_64-linux;
    in {
      packages.x86_64-linux.default = pkgs.rustPlatform.buildRustPackage rec {
        pname = "swww";
        version = "5.0.0";
        src = ./.;

        cargoLock.lockFile = ./Cargo.lock;
        buildType = "release";
        doCheck = false; # Fails to connect to socket during build

        nativeBuildInputs = with pkgs; [ pkg-config ];

        buildInputs = with pkgs; [ libxkbcommon lz4 ];
      };

      overlays = {
        swww = _: prev: { swww = self.packages.x86_64-linux.swww; };
        default = self.overlays.swww;
      };
      devShells.x86_64-linux.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          pkg-config
          libxkbcommon
          lz4
          rustc
          cargo
          rust-analyzer
        ];
      };

      homeManagerModules.default = { config, lib, pkgs, ... }:
        let cfg = config.programs.swww;
        in {
          options.programs.swww = {
            enable = lib.mkEnableOption
              "swww, a solution to your wayland wallpaper woes";
            package = lib.mkOption {
              type = lib.type.package;
              default = self.packages.x86_64-linux.default;
            };
            systemd = {
              enable = lib.mkEnableOption "Enable systemd integration";
              target = lib.mkOption {
                type = lib.types.str;
                default = "graphical-session.target";
              };
            };
          };
          config = lib.mkIf cfg.enable (lib.mkMerge [
            { home.packages = lib.optional (cfg.package != null) cfg.package; }

            (lib.mkIf cfg.systemd.enable {
              systemd.user.services.swww = {
                Unit = {
                  Description =
                    "swww, a solution to your wayland wallpaper woes";
                  Documentation = "https://github.com/Hourus645/swww";
                  PartOf = [ "graphical-session.target" ];
                  After = [ "graphical-session.target" ];
                };

                Service = {
                  ExecStart = "${cfg.package}/bin/swww init --no-daemon";
                  ExecReload = "${pkgs.coreutils}/bin/kill -SIGUSR2 $MAINPID";
                  Restart = "on-failure";
                  KillMode = "mixed";
                };
                Install = { WantedBy = [ cfg.systemd.target ]; };
              };
            })
          ]);
        };
    };
}
