{
  config,
  lib,
  pkgs,
  ...
}: let
  owned = {
    owner = "garage";
    group = "garage";
  };

  buckets = {
    velero.keys = {
      velero = ["read" "write"];
      offsite = ["read"];
    };
    nix-cache = {
      website = true;
      quota = "200GiB";
      keys.ci-nix-cache = ["read" "write"];
    };
  };

  used = lib.unique (lib.concatMap (b: lib.attrNames (b.keys or {})) (lib.attrValues buckets));

  invalid = lib.concatMap (b:
    lib.subtractLists ["website" "quota" "keys"] (lib.attrNames b)
    ++ lib.subtractLists ["read" "write" "owner"] (lib.concatLists (lib.attrValues (b.keys or {}))))
  (lib.attrValues buckets);

  grants = lib.concatLists (lib.mapAttrsToList (bucket: b:
    lib.mapAttrsToList (key: perms: "[${lib.escapeShellArg "${bucket}/${key}"}]=${lib.escapeShellArg (lib.concatStringsSep " " perms)}") (b.keys or {}))
  buckets);

  bucketCall = name: b: let
    bucket = lib.escapeShellArg name;
    website =
      if b.website or false
      then "allow"
      else "deny";
  in ''
    garage bucket info ${bucket} > /dev/null 2>&1 || garage bucket create ${bucket} > /dev/null
    garage bucket website --${website} ${bucket} > /dev/null
    garage bucket set-quotas --max-size ${lib.escapeShellArg (b.quota or "none")} ${bucket} > /dev/null
  '';

  garage-provision = pkgs.writeShellApplication {
    name = "garage-provision";
    runtimeInputs = [config.services.garage.package];
    text = ''
      keyfile=${config.secrets.garage_keys.path}
      buckets=(${lib.escapeShellArgs (lib.attrNames buckets)})
      declare -A used=(${lib.concatMapStringsSep " " (key: "[${lib.escapeShellArg key}]=1") used})
      declare -A perms=(${lib.concatStringsSep " " grants})
      declare -A ids secrets seen
      line=0

      while read -r name id secret || [ -n "$name" ]; do
        line=$((line + 1))
        if [ -z "$name" ]; then continue; fi
        if [[ ! $id =~ ^GK[0-9a-f]{24}$ || ! $secret =~ ^[0-9a-f]{64}$ ]]; then
          echo "line $line of $keyfile is invalid" >&2
          exit 1
        fi
        if [ -n "''${ids[$name]:-}" ] || [ -n "''${seen[$id]:-}" ]; then
          echo "line $line of $keyfile is a duplicate" >&2
          exit 1
        fi
        ids[$name]=$id
        secrets[$name]=$secret
        seen[$id]=1
      done < "$keyfile"

      for name in "''${!used[@]}"; do
        if [ -z "''${ids[$name]:-}" ]; then
          echo "key $name is not in $keyfile" >&2
          exit 1
        fi
      done

      tries=0
      until garage bucket list > /dev/null 2>&1 || [ $((tries += 1)) -ge 60 ]; do sleep 1; done
      garage bucket list > /dev/null

      ${lib.concatStrings (lib.mapAttrsToList bucketCall buckets)}
      for name in "''${!used[@]}"; do
        id=''${ids[$name]}
        if ! garage key info "$id" > /dev/null 2>&1; then
          garage key import --yes -n "$name" "$id" "''${secrets[$name]}" > /dev/null
          echo "imported key $name"
        fi
        for bucket in "''${buckets[@]}"; do
          allow=()
          deny=()
          for perm in read write owner; do
            case " ''${perms[$bucket/$name]:-} " in
              *" $perm "*) allow+=("--$perm") ;;
              *) deny+=("--$perm") ;;
            esac
          done
          if [ "''${#allow[@]}" -gt 0 ]; then garage bucket allow "''${allow[@]}" "$bucket" --key "$id" > /dev/null; fi
          if [ "''${#deny[@]}" -gt 0 ]; then garage bucket deny "''${deny[@]}" "$bucket" --key "$id" > /dev/null; fi
        done
      done

      for name in "''${!ids[@]}"; do
        if [ -n "''${used[$name]:-}" ]; then continue; fi
        id=''${ids[$name]}
        if garage key info "$id" > /dev/null 2>&1; then
          garage key delete --yes "$id" > /dev/null
          echo "deleted key $name"
        fi
      done
    '';
  };
in {
  assertions = [
    {
      assertion = invalid == [];
      message = "garage buckets: unknown attributes or permissions: ${toString invalid}";
    }
  ];

  secrets.garage_rpc_secret = owned;
  secrets.garage_keys = owned // {restartUnits = ["garage-provision.service"];};

  users.users.garage = {
    isSystemUser = true;
    group = "garage";
  };
  users.groups.garage = {};

  # The S3 and web ports are not opened in the firewall: they are reachable only over the
  # trusted tailscale0 and cni0 interfaces, the LAN is blocked.
  services.garage = {
    enable = true;
    package = pkgs.garage_2;
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
      s3_web = {
        bind_addr = "0.0.0.0:3902";
        root_domain = ".web.garage";
      };
    };
  };

  systemd = {
    # A static user, so the secret and the dataset can be owned by it
    services.garage = {
      # The fstab mount of tank/garage races zfs-mount.service at boot and loses
      # (the dataset ends up mounted anyway), so RequiresMountsFor would fail
      # garage. Wait for zfs-mount instead and refuse to start on the bare root.
      after = ["zfs-mount.service"];
      wants = ["zfs-mount.service"];
      serviceConfig = {
        ExecStartPre = "${pkgs.util-linux}/bin/mountpoint -q /mnt/garage";
        Restart = "on-failure";
        RestartSec = "30s";
        MemoryHigh = "1G";
        MemoryMax = "1536M";
        DynamicUser = false;
        User = "garage";
        Group = "garage";
        ExecStart = lib.mkForce "${config.services.garage.package}/bin/garage server --single-node";
      };
    };

    services.garage-provision = {
      wantedBy = ["multi-user.target"];
      requires = ["garage.service"];
      after = ["garage.service"];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        TimeoutStartSec = "2min";
        User = "garage";
        Group = "garage";
        ExecStart = lib.getExe garage-provision;
      };
    };

    tmpfiles.rules = [
      "d /mnt/garage/meta 0700 garage garage -"
      "d /mnt/garage/data 0700 garage garage -"
    ];
  };
}
