{
  config,
  lib,
  pkgs,
  ...
}: let
  stamp = "/var/lib/offsite-backup/last-success";

  condition = pkgs.writeShellApplication {
    name = "offsite-backup-condition";
    runtimeInputs = [pkgs.coreutils];
    text = ''
      [[ -e /dev/disk/by-partlabel/offsite ]] || exit 1

      hour=$(date +%-H)
      if ((hour >= 17 && hour < 20)); then
        exit 1
      fi

      if [[ -e ${stamp} ]] && (($(date +%s) - $(stat -c %Y ${stamp}) < 20 * 3600)); then
        exit 1
      fi
    '';
  };

  stale = pkgs.writeShellApplication {
    name = "offsite-backup-stale";
    runtimeInputs = [pkgs.coreutils pkgs.jq pkgs.event-bus];
    text = ''
      if [[ -e ${stamp} ]]; then
        days=$((($(date +%s) - $(stat -c %Y ${stamp})) / 86400))
        ((days >= 7)) || exit 0
        title="No offsite backup for $days days"
      else
        title="No offsite backup yet"
      fi

      jq -n --arg host ${config.networking.hostName} --arg title "$title" \
        '{host: $host, title: $title, body: "Plug in the backup drive (label offsite)"}' |
        event-pub backup.offsite_stale
    '';
  };
in {
  secrets = {
    offsite_key = {};
    offsite_s3_env = {};
  };

  environment.etc.crypttab.text = "offsite /dev/disk/by-partlabel/offsite ${config.secrets.offsite_key.path} luks,noauto,nofail,x-systemd.device-timeout=10s";

  fileSystems = {
    "/mnt/offsite" = {
      device = "/dev/mapper/offsite";
      fsType = "btrfs";
      options = [
        "noauto"
        "nofail"
        "nosuid"
        "nodev"
        "noexec"
        "x-systemd.automount"
        "x-systemd.idle-timeout=5min"
        "x-systemd.device-timeout=30s"
      ];
    };
    "/mnt/regis/photos" = {
      device = "${config.tailnet.ips.regis}:/mnt/photos";
      fsType = "nfs";
      options = [
        "ro"
        "vers=4.2"
        "soft"
        "nosuid"
        "nodev"
        "noexec"
        "noauto"
        "x-systemd.automount"
        "x-systemd.idle-timeout=10min"
      ];
    };
  };

  systemd = {
    tmpfiles.rules = ["d /mnt/regis 0700 root root -"];

    services.restic-backups-offsite = {
      wantedBy = ["dev-disk-by\\x2dpartlabel-offsite.device"];
      environment = {
        RCLONE_CONFIG_GARAGE_TYPE = "s3";
        RCLONE_CONFIG_GARAGE_PROVIDER = "Other";
        RCLONE_CONFIG_GARAGE_ENDPOINT = "http://${config.tailnet.ips.regis}:3900";
        RCLONE_CONFIG_GARAGE_REGION = "garage";
        RCLONE_CONFIG_GARAGE_ENV_AUTH = "true";
      };
      serviceConfig = {
        StateDirectory = "offsite-backup";
        ExecCondition = lib.getExe condition;
        ExecStartPost = "${pkgs.coreutils}/bin/touch ${stamp}";
      };
    };

    user = {
      services.offsite-backup-stale = {
        description = "Notify when the offsite backup is overdue";
        after = ["event-handlers.service"];
        unitConfig.ConditionUser = "nikodem";
        serviceConfig = {
          Type = "oneshot";
          ExecStart = lib.getExe stale;
        };
      };
      timers.offsite-backup-stale = {
        wantedBy = ["timers.target"];
        timerConfig = {
          OnStartupSec = "15min";
          OnCalendar = "daily";
          Persistent = true;
        };
      };
    };
  };

  services.restic.backups.offsite = {
    repository = "/mnt/offsite/restic";
    passwordFile = config.secrets.offsite_key.path;
    environmentFile = config.secrets.offsite_s3_env.path;
    initialize = false;
    paths = ["/mnt/offsite/staging/velero" "/mnt/regis/photos"];
    pruneOpts = ["--keep-daily 7" "--keep-weekly 4" "--keep-monthly 6"];
    checkOpts = ["--read-data-subset=5%"];
    inhibitsSleep = true;
    timerConfig.OnCalendar = "hourly";
    backupPrepareCommand = ''
      set -e
      test -e /mnt/offsite/restic/config
      ${lib.getExe pkgs.rclone} lsf --max-depth 1 garage:velero | grep -q .
      test -n "$(ls -A /mnt/regis/photos)"
      chmod 0700 /mnt/offsite
      install -d -m 0700 /mnt/offsite/staging/velero
      ${lib.getExe pkgs.rclone} sync garage:velero /mnt/offsite/staging/velero
    '';
    backupCleanupCommand = ''
      [[ $SERVICE_RESULT == exec-condition ]] && exit 0
      systemctl stop mnt-offsite.mount systemd-cryptsetup@offsite.service
    '';
  };

  persist.generated.directories = ["/var/lib/offsite-backup"];
}
