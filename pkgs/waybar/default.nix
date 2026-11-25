{
  pkgs,
  waybar,
  ...
}: let
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
  agent-status = pkgs.writeShellScriptBin "agent-status" ''
    jw session list | jq -sc '
      map(select(.running and .status != "done")) as $s
      | ($s | map(select(.status == "busy")) | length) as $busy
      | if $s == [] then {text: ""} else {
          text: "\($busy)/\($s | length)",
          alt: (if $busy > 0 then "busy" else "idle" end),
          class: (if $busy > 0 then "busy" else "idle" end),
          tooltip: ($s | map("\(.status)  \(.title // .name)") | join("\n") | gsub("&"; "&amp;") | gsub("<"; "&lt;") | gsub(">"; "&gt;"))
        } end'
  '';
in
  pkgs.symlinkJoin {
    name = "waybar";
    paths = [waybar];
    buildInputs = [pkgs.makeWrapper];
    passthru = {inherit wireguard-status wireguard-toggle;};
    postBuild = let
      extraPkgs = [
        pkgs.iproute2
        pkgs.jq
        wireguard-status
        wireguard-toggle
        agent-status
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
