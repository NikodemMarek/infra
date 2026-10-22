{
  pkgs,
  lib,
  config,
  ...
}: let
  cfg = config.services.event-router;
  eventRouter = lib.getExe pkgs.event-router;

  # without this shpool types a prompt-setup command into the session's stdin,
  # which for the event handler's sessions is Claude's input; it applies to the
  # user's manual sessions too
  shpoolConfig = pkgs.writeText "shpool.toml" ''
    prompt_prefix = ""
  '';
in {
  options.services.event-router.enable = lib.mkEnableOption "the event router, the knowledge curator timer and the idle Claude session reaper";

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [pkgs.shpool];

    # the event handler for claude sessions runs them in shpool sessions
    systemd.user.sockets.shpool = {
      description = "Shpool session daemon socket";
      wantedBy = ["sockets.target"];
      listenStreams = ["%t/shpool/shpool.socket"];
      socketConfig.SocketMode = "0600";
    };

    systemd.user.services = {
      # restarting it kills every pooled session
      shpool = {
        description = "Shpool session daemon";
        requires = ["shpool.socket"];
        serviceConfig = {
          ExecStart = "${pkgs.shpool}/bin/shpool --config-file ${shpoolConfig} daemon";
          KillMode = "mixed";
          TimeoutStopSec = "2s";
          SendSIGHUP = true;
        };
      };

      event-router = {
        description = "Dispatch events to handlers";
        wantedBy = ["default.target"];
        after = ["graphical-session.target"];
        serviceConfig = {
          ExecStart = "${eventRouter} serve";
          Restart = "on-failure";
          RestartSec = 10;
          # handlers may leave detached processes (a notification waiting for
          # its action); a restart must not take them down
          KillMode = "process";
        };
      };

      knowledge-curate = {
        description = "Merge the pending knowledge notes into the knowledge store";
        environment.KNOWLEDGE_CLAUDE = "${pkgs.claude}/bin/claude-bare";
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${lib.getExe pkgs.knowledge} curate";
        };
        startAt = "*-*-* 00/6:15:00";
      };

      jw-reap = {
        description = "Stop detached Claude shpool sessions idle or stuck on a permission prompt for 4 hours";
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${pkgs.jw}/bin/jw reap";
          TimeoutStartSec = "2min";
        };
        startAt = "hourly";
      };
    };

    systemd.user.timers.knowledge-curate.timerConfig = {
      Persistent = true;
      RandomizedDelaySec = "10min";
    };
    systemd.user.timers.jw-reap.timerConfig.Persistent = true;
  };
}
