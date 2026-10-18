{
  lib,
  pkgs,
  ...
}: let
  stamp = "/run/idle-suspend-since";

  idle-suspend = pkgs.writeShellApplication {
    name = "idle-suspend";
    runtimeInputs = [
      pkgs.conntrack-tools
      pkgs.systemd
      pkgs.coreutils
    ];
    text = ''
      stamp=${stamp}
      limit=1800
      reason=""
      errfile=$(mktemp)
      trap 'rm -f "$errfile"' EXIT

      # Wolf's video port: Moonlight pings it every 500 ms while streaming.
      # Only stdout counts, the "N flow entries" summary goes to stderr.
      for family in ipv4 ipv6; do
        rc=0
        flows=$(conntrack -f "$family" -L -p udp --dport 48100 2>"$errfile") || rc=$?
        if ((rc != 0)); then
          reason="probe failed: conntrack $family exit $rc"
          echo "$reason: $(<"$errfile")" >&2
          break
        fi
        if [[ -n $flows ]]; then
          reason=stream
          break
        fi
      done

      # Closing sessions linger forever when a user leaves background processes behind.
      if [[ -z $reason ]]; then
        rc=0
        sessions=$(loginctl list-sessions --no-legend 2>"$errfile") || rc=$?
        if ((rc != 0)); then
          reason="probe failed: loginctl list-sessions exit $rc"
          echo "$reason: $(<"$errfile")" >&2
        else
          while read -r id _; do
            [[ -n $id ]] || continue
            # The session may have ended since the listing.
            info=$(loginctl show-session "$id" -p Service -p State -p Name 2>/dev/null) || continue
            service="" state="" user=""
            while IFS="=" read -r key value; do
              case $key in
                Service) service=$value ;;
                State) state=$value ;;
                Name) user=$value ;;
              esac
            done <<<"$info"
            if [[ $service == sshd && $state != closing ]]; then
              reason="ssh session $id ($user)"
              break
            fi
          done <<<"$sessions"
        fi
      fi

      if [[ -n $reason ]]; then
        if [[ -e $stamp ]]; then
          rm -f "$stamp"
          echo "active again: $reason"
        fi
        exit 0
      fi

      if [[ ! -e $stamp ]]; then
        touch "$stamp"
        echo "idle, suspending in 30 min"
        exit 0
      fi

      since=$(stat -c %Y "$stamp")
      now=$(date +%s)
      if ((now - since < limit)); then
        exit 0
      fi

      rm -f "$stamp"
      echo "idle for 30 min, suspending"
      systemctl suspend-then-hibernate || systemctl suspend
    '';
  };
in {
  systemd.services.idle-suspend = {
    description = "Suspend when there is no Moonlight stream or SSH session";
    serviceConfig = {
      Type = "oneshot";
      ExecStart = lib.getExe idle-suspend;
    };
  };

  systemd.timers.idle-suspend = {
    wantedBy = ["timers.target"];
    timerConfig = {
      OnBootSec = "1min";
      OnUnitActiveSec = "1min";
    };
  };

  # A suspend not started by the script must not leave a stale stamp behind.
  powerManagement.resumeCommands = "rm -f ${stamp}";
}
