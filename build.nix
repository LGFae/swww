{
  lib,
  rustPlatform,
  pkg-config,
  lz4,
  libxkbcommon,
  installShellFiles,
  scdoc,
  nix-gitignore,
  wayland,
  wayland-protocols,
}: let
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
  src = nix-gitignore.gitignoreSource [] ./.;

  # HACK: waybackend and pkg-config try to find wayland.xml in pkgs.wayland,
  # but wayland.xml is not included in the package.
  wayland' = wayland.overrideAttrs {
    postInstall = ''
      mkdir -p $out/share/wayland
      install ../protocol/wayland.xml -t $out/share/wayland/
    '';
  };
in
  rustPlatform.buildRustPackage {
    pname = "swww";

    inherit src version;

    cargoLock.lockFile = ./Cargo.lock;

    buildInputs = [
      lz4
      libxkbcommon
      wayland'
      wayland-protocols
    ];

    doCheck = false; # Integration tests do not work in sandbox environment

    nativeBuildInputs = [
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
  }
