# To use this, first create a new key for the disks and copy it via nixos-anywhere (uncomment line).
# If this does not work, copy the file to the installation medium directly.
# dd if=/dev/urandom of=host/regis/disk.key bs=32 count=1 status=none
{
  disko.devices = {
    disk = {
      # -------------------------------------------------------------
      # 1TB SSD
      # -------------------------------------------------------------
      ssd = {
        type = "disk";
        device = "/dev/disk/by-id/nvme-Samsung_SSD_990_PRO_1TB_S7HDNX0L425194H";
        content = {
          type = "gpt";
          partitions = {
            ESP = {
              size = "1G";
              type = "EF00";
              content = {
                type = "filesystem";
                format = "vfat";
                mountpoint = "/boot";
                mountOptions = ["umask=0077"];
              };
            };
            root = {
              size = "100%";
              content = {
                type = "zfs";
                pool = "rpool";
              };
            };
          };
        };
      };

      # -------------------------------------------------------------
      # 18TB HDDs
      # -------------------------------------------------------------
      hdd-18a = {
        type = "disk";
        device = "/dev/disk/by-id/ata-ST18000NE000-3G6101_ZVTCGEA9";
        content = {
          type = "gpt";
          partitions = {
            zfs = {
              size = "100%";
              content = {
                type = "zfs";
                pool = "tank";
              };
            };
          };
        };
      };

      hdd-18b = {
        type = "disk";
        device = "/dev/disk/by-id/ata-ST18000NE000-3G6101_ZVTCD1B6";
        content = {
          type = "gpt";
          partitions = {
            zfs = {
              size = "100%";
              content = {
                type = "zfs";
                pool = "tank";
              };
            };
          };
        };
      };
    };

    # ---------------------------------------------------------------
    # ZFS Pools & Encrypted Datasets
    # ---------------------------------------------------------------
    zpool = {
      # OS Root Pool (Encrypted - Prompt for Passphrase on Boot)
      rpool = {
        type = "zpool";
        options = {
          ashift = "12";
          autotrim = "on";
        };
        rootFsOptions = {
          compression = "zstd";
          acltype = "posixacl";
          xattr = "sa";
          atime = "off";
          encryption = "aes-256-gcm";
          keyformat = "passphrase";
          keylocation = "prompt";
        };
        datasets = {
          "root" = {
            type = "zfs_fs";
            mountpoint = "/";
            postCreateHook = "zfs snapshot rpool/root@blank";
          };
          "nix" = {
            type = "zfs_fs";
            mountpoint = "/nix";
            options = {
              atime = "off";
            };
          };
          "persist" = {
            type = "zfs_fs";
            mountpoint = "/persist";
          };
          "openebs" = {
            type = "zfs_fs";
            mountpoint = "/mnt/openebs";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
            };
          };
          "models" = {
            type = "zfs_fs";
            mountpoint = "/mnt/models";
            mountOptions = ["nofail"];
            options = {
              recordsize = "1M";
              overlay = "on";
              atime = "off";
            };
          };
        };
      };

      # Data Storage Pool (Encrypted via Keyfile stored on rpool)
      tank = {
        type = "zpool";
        mountOptions = ["nofail"];
        options = {
          ashift = "12";
          overlay = "on";
        };
        mode = {
          topology = {
            type = "topology";
            vdev = [
              {
                mode = "mirror";
                members = ["hdd-18a" "hdd-18b"];
              }
            ];
          };
        };
        rootFsOptions = {
          compression = "zstd";
          acltype = "posixacl";
          xattr = "sa";
          atime = "off";
          mountpoint = "none";
          encryption = "aes-256-gcm";
          keyformat = "raw";
          keylocation = "file:///tmp/disk.key";
        };
        postCreateHook = ''
          zfs set keylocation=file:///persist/data/etc/zfs/disk.key tank
        '';
        datasets = {
          "music" = {
            type = "zfs_fs";
            mountpoint = "/mnt/music";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
            };
          };
          "movies" = {
            type = "zfs_fs";
            mountpoint = "/mnt/movies";
            mountOptions = ["nofail"];
            options = {
              recordsize = "1M";
              overlay = "on";
              atime = "off";
            };
          };
          "shows" = {
            type = "zfs_fs";
            mountpoint = "/mnt/shows";
            mountOptions = ["nofail"];
            options = {
              recordsize = "1M";
              overlay = "on";
              atime = "off";
            };
          };
          "books" = {
            type = "zfs_fs";
            mountpoint = "/mnt/books";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
            };
          };
          "photos" = {
            type = "zfs_fs";
            mountpoint = "/mnt/photos";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
            };
          };
          "files" = {
            type = "zfs_fs";
            mountpoint = "/mnt/files";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
            };
          };
          "backups" = {
            type = "zfs_fs";
            mountpoint = "/mnt/backups";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
            };
          };
          "garage" = {
            type = "zfs_fs";
            mountpoint = "/mnt/garage";
            mountOptions = ["nofail"];
            options = {
              recordsize = "128K";
              overlay = "on";
              atime = "off";
              quota = "1T";
            };
          };
        };
      };
    };
  };
}
