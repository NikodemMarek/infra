{
  pkgs,
  hyprland,
  ...
}:
pkgs.symlinkJoin {
  name = "Hyprland";
  paths = [hyprland];
  buildInputs = [pkgs.makeWrapper];
  inherit (hyprland) passthru version;
  meta = {
    inherit (hyprland.meta) description homepage license platforms;
    mainProgram = "Hyprland";
  };
  postBuild = let
    extraPkgs = [
      pkgs.hypridle
      pkgs.hyprlock
      pkgs.hyprpaper
      pkgs.rofi
      pkgs.dunst
      pkgs.waybar
      pkgs.kanshi

      pkgs.yazi
      pkgs.zen-browser
      pkgs.alacritty
      pkgs.wl-clipboard
      pkgs.grim
      pkgs.slurp
      pkgs.pulseaudio
      pkgs.brightnessctl
      pkgs.wdisplays
      pkgs.wl-mirror
    ];
  in ''
    wrapProgram $out/bin/Hyprland \
        --suffix PATH : ${pkgs.lib.strings.makeBinPath extraPkgs} \
        --add-flags "--config $out/config/hypr/config.lua"

    mkdir -p $out/config/hypr
    cp ${./config.lua} $out/config/hypr/config.lua
  '';
}
