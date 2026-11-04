# run with: sudo nix --experimental-features "nix-command flakes" run github:nix-community/disko -- --mode disko ./btrfs-persistence-swapfile.nix --arg device '"/dev/sda"' --arg swap 'number'
{
  device,
  swap ? 4,
  ...
}: {
  disko.devices = {
    disk = {
      main = {
        inherit device;
        type = "disk";
        content = {
          type = "gpt";
          partitions = {
            esp = {
              priority = 1;
              name = "boot";
              size = "500M";
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
                type = "btrfs";
                extraArgs = ["-f"];

                subvolumes = {
                  "/root" = {
                    mountpoint = "/";
                  };

                  "/swap" = {
                    mountpoint = "/.swapvol";
                    swap.swapfile.size = "${builtins.toString swap}G";
                  };

                  "/nix" = {
                    mountOptions = ["subvol=nix" "compress=zstd" "noatime"];
                    mountpoint = "/nix";
                  };

                  "/persist" = {
                    mountOptions = ["subvol=persist" "compress=zstd"];
                    mountpoint = "/persist";
                  };
                };
              };
            };
          };
        };
      };
    };
  };
}
