{
  config,
  lib,
  pkgs,
  ...
}: {
  secrets.garage_rpc_secret = {
    owner = "garage";
    group = "garage";
  };
  secrets.garage_env.profile = "dijkstra-velero";

  users.users.garage = {
    isSystemUser = true;
    group = "garage";
  };
  users.groups.garage = {};

  # The S3 port is not opened in the firewall: it is reachable only over the
  # trusted tailscale0 and cni0 interfaces, the LAN is blocked.
  services.garage = {
    enable = true;
    package = pkgs.garage_2;
    environmentFile = config.secrets.garage_env.path;
    extraEnvironment.GARAGE_DEFAULT_BUCKET = "velero";
    settings = {
      metadata_dir = "/mnt/garage/meta";
      data_dir = "/mnt/garage/data";
      # Single-node LMDB can corrupt on an unclean shutdown
      metadata_auto_snapshot_interval = "6h";
      replication_factor = 1;
      rpc_bind_addr = "127.0.0.1:3901";
      rpc_public_addr = "127.0.0.1:3901";
      rpc_secret_file = config.secrets.garage_rpc_secret.path;
      s3_api = {
        api_bind_addr = "0.0.0.0:3900";
        s3_region = "garage";
      };
    };
  };

  # A static user, so the secret and the dataset can be owned by it
  systemd.services.garage = {
    # The fstab mount of tank/garage races zfs-mount.service at boot and loses
    # (the dataset ends up mounted anyway), so RequiresMountsFor would fail
    # garage. Wait for zfs-mount instead and refuse to start on the bare root.
    after = ["zfs-mount.service"];
    wants = ["zfs-mount.service"];
    serviceConfig = {
      ExecStartPre = "${pkgs.util-linux}/bin/mountpoint -q /mnt/garage";
      Restart = "on-failure";
      RestartSec = "30s";
      DynamicUser = false;
      User = "garage";
      Group = "garage";
      # Creates the single-node layout, the bucket velero and Velero's key
      # (from garage_env) on first start
      ExecStart = lib.mkForce "${config.services.garage.package}/bin/garage server --single-node --default-bucket";
    };
  };

  systemd.tmpfiles.rules = [
    "d /mnt/garage/meta 0700 garage garage -"
    "d /mnt/garage/data 0700 garage garage -"
  ];
}
