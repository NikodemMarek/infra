{
  pkgs,
  waybar,
  ...
}:
pkgs.symlinkJoin {
  name = "waybar";
  paths = [waybar];
  buildInputs = [pkgs.makeWrapper];
  postBuild = let
    wireguard-status = pkgs.writeShellScriptBin "wireguard-status" ''
      if ip -j link show wg0 2> /dev/null | jq -e '.[0].flags | index("UP")' > /dev/null; then
        echo '{"text":"wg0","alt":"on","tooltip":"WireGuard: Mullvad","class":"vpn-on","percentage":100}'
      else
        echo '{"text":"off","alt":"off","tooltip":"WireGuard: off","class":"vpn-off","percentage":0}'
      fi
    '';
    wireguard-toggle = pkgs.writeShellScriptBin "wireguard-toggle" ''
      STATUS=$(wireguard-status | jq .percentage)
      if [ $STATUS -ge 50 ]; then
        sudo -n /run/current-system/sw/bin/networkctl down wg0
      else
        sudo -n /run/current-system/sw/bin/networkctl up wg0
      fi
    '';
    extraPkgs = [
      pkgs.iproute2
      pkgs.jq
      wireguard-status
      wireguard-toggle
    ];
  in ''
    wrapProgram $out/bin/waybar \
        --suffix PATH : ${pkgs.lib.strings.makeBinPath extraPkgs} \
        --add-flags "--config $out/config/waybar/config.json" \
        --add-flags "--style $out/config/waybar/style.css"

    mkdir -p $out/config/waybar
    cp ${./config.json} $out/config/waybar/config.json
    cp ${./colors.css} $out/config/waybar/colors.css
    cp ${./style.css} $out/config/waybar/style.css
  '';
}
