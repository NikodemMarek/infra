{
  pkgs,
  quickshell,
  ...
}: let
  panel-status = pkgs.writeShellScriptBin "panel-status" ''
    awake=false
    stay-awake status > /dev/null && awake=true
    wg=$(wireguard-status | jq '.percentage >= 50')
    wifi=$(networkctl list --json=short | jq -c 'first(.Interfaces[] | select(.Type == "wlan") | {ssid: (.SSID // ""), state: .OperationalState}) // {}')
    volume=$(pactl get-sink-volume @DEFAULT_SINK@ | grep -o '[0-9]*%' | head -1 | tr -d %)
    muted=false
    pactl get-sink-mute @DEFAULT_SINK@ | grep -q yes && muted=true
    bluetooth=$(rfkill -J | jq 'any(.rfkilldevices[]; .type == "bluetooth" and .soft == "unblocked")')
    brightness=$(brightnessctl -c backlight -m | cut -d, -f4 | tr -d %)
    agents=$(jw session list | jq -sc 'map(select(.running and .status != "done") | {status, title: (.title // .name)})')
    tasks=$(tm list --json | jq -c 'map(select(.status != "done") | {line, status, description})')
    jq -nc \
      --argjson awake "$awake" --argjson wg "$wg" --argjson wifi "$wifi" \
      --argjson volume "''${volume:-0}" --argjson muted "$muted" --argjson bluetooth "$bluetooth" --argjson brightness "''${brightness:-0}" \
      --argjson agents "''${agents:-[]}" --argjson tasks "''${tasks:-[]}" \
      '$ARGS.named'
  '';
  extraPkgs = [
    pkgs.jq
    pkgs.gnugrep
    pkgs.iproute2
    pkgs.util-linux
    pkgs.pulseaudio
    pkgs.brightnessctl
    pkgs.stay-awake
    pkgs.waybar.wireguard-status
    pkgs.waybar.wireguard-toggle
    panel-status
  ];
in
  pkgs.symlinkJoin {
    name = "quickshell";
    paths = [quickshell];
    buildInputs = [pkgs.makeWrapper];
    inherit (quickshell) meta;
    postBuild = ''
      wrapProgram $out/bin/quickshell \
          --suffix PATH : ${pkgs.lib.strings.makeBinPath extraPkgs} \
          --add-flags "-p $out/config/quickshell/shell.qml"

      mkdir -p $out/config/quickshell
      colors=$(sed -nE 's/@define-color ([a-z0-9]+) (#[0-9a-f]+);/\1: "\2",/p' ${../waybar/colors.css} | tr -d "\n")
      substitute ${./shell.qml} $out/config/quickshell/shell.qml --replace-fail @colors@ "$colors"
    '';
  }
