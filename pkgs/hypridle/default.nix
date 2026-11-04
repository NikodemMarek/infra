{
  pkgs,
  hypridle,
  ...
}:
pkgs.symlinkJoin {
  name = "hypridle";
  paths = [hypridle];
  buildInputs = [pkgs.makeWrapper];
  postBuild = let
    extraPkgs = [pkgs.hyprlock pkgs.brightnessctl];
  in ''
    wrapProgram $out/bin/hypridle \
        --suffix PATH : ${pkgs.lib.strings.makeBinPath extraPkgs} \
        --set XDG_CONFIG_HOME $out/.config \
        --add-flags "--config $out/config/hypridle/config.conf"

    mkdir -p $out/config/hypridle
    cp ${./config.conf} $out/config/hypridle/config.conf

    # hypridle complains when the default config path is missing, even with --config
    mkdir -p $out/.config/hypr
    touch $out/.config/hypr/hypridle.conf
  '';
}
