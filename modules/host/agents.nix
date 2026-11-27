{
  pkgs,
  lib,
  config,
  ...
}: let
  cfg = config.services.agents;

  # without this shpool types a prompt-setup command into the session's stdin,
  # which for jw sessions is Claude's input; it applies to the user's manual
  # sessions too
  shpoolConfig = pkgs.writeText "shpool.toml" ''
    prompt_prefix = ""
  '';

  jwReapPath = lib.makeBinPath [
    cfg.jwPackage
    cfg.ghPackage
    cfg.glabPackage
    pkgs.jujutsu
    pkgs.git
    pkgs.openssh
  ];
in {
  options.services.agents = {
    enable = lib.mkEnableOption "the local NATS server, the event handlers and the idle Claude session reaper";
    jwPackage = lib.mkPackageOption pkgs "jw" {};
    ghPackage = lib.mkPackageOption pkgs "gh" {};
    glabPackage = lib.mkPackageOption pkgs "glab" {};
    user = lib.mkOption {
      type = lib.types.str;
      description = "The user whose rootless podman runs the containers; needs a fixed uid and lingering";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [pkgs.shpool pkgs.natscli pkgs.plan-review];

    virtualisation.oci-containers.containers.nats = {
      imageStream = pkgs.nats-server-image;
      image = "${pkgs.nats-server-image.imageName}:${pkgs.nats-server-image.imageTag}";
      cmd = ["--name" config.networking.hostName];
      volumes = ["nats:/data"];
      networks = ["host"];
      podman.user = cfg.user;
    };

    systemd = {
      # units ship with the packages; NixOS ignores their [Install]
      packages = [pkgs.event-handlers pkgs.plan-review];

      user = {
        # jw runs agent sessions in shpool sessions
        sockets.shpool = {
          description = "Shpool session daemon socket";
          wantedBy = ["sockets.target"];
          listenStreams = ["%t/shpool/shpool.socket"];
          socketConfig.SocketMode = "0600";
        };

        services = {
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

          event-handlers = {
            wantedBy = ["default.target"];
            environment = {
              PATH = lib.mkForce "/run/wrappers/bin:/etc/profiles/per-user/%u/bin:/run/current-system/sw/bin";
              SSH_AUTH_SOCK = "%t/ssh-agent";
            };
          };

          plan-review.wantedBy = ["default.target"];

          jw-reap = {
            description = "Publish MR activity of agent sessions, stop finished or long-idle ones and remove their merged workspaces";
            environment = {
              PATH = lib.mkForce "${jwReapPath}:/run/wrappers/bin:/etc/profiles/per-user/%u/bin:/run/current-system/sw/bin";
              SSH_AUTH_SOCK = "%t/ssh-agent";
            };
            serviceConfig = {
              Type = "oneshot";
              ExecStart = [
                "-${cfg.jwPackage}/bin/jw poll"
                "${cfg.jwPackage}/bin/jw reap"
              ];
              TimeoutStartSec = "20min";
            };
            startAt = "*:0/3";
          };
        };

        timers = {
          jw-reap.timerConfig.Persistent = true;
        };
      };
    };
  };
}
