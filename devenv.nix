{pkgs, ...}: {
  # https://devenv.sh/packages/
  packages = with pkgs; [
    ffmpeg
  ];

  # https://devenv.sh/languages/
  languages.rust.enable = true;
}
