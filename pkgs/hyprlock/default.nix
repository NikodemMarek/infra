{
  pkgs,
  hyprlock,
  wallpaper,
  ...
}:
pkgs.symlinkJoin {
  name = "hyprlock";
  paths = [hyprlock];
  buildInputs = [pkgs.makeWrapper];
  postBuild = ''
    wrapProgram $out/bin/hyprlock \
        --set CONF $out/config/hyprlock \
        --add-flags "--config ${./config.conf}"

    mkdir -p $out/config/hyprlock
    cp ${./colors.conf} $out/config/hyprlock/colors.conf
    cp ${wallpaper} $out/config/hyprlock/background.png
  '';
}
