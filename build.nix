{
  lib,
  rustPlatform,
  pkg-config,
  lz4,
  libxkbcommon,
  installShellFiles,
  scdoc,
  nix-gitignore,
}: let
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
  src = nix-gitignore.gitignoreSource [] ./.;
in
rustPlatform.buildRustPackage {
  pname = "swww";

  inherit src version;

  cargoLock.lockFile = ./Cargo.lock;

  buildInputs = [
    lz4
    libxkbcommon
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
