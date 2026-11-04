# run with: sudo nix --experimental-features "nix-command flakes" run github:nix-community/disko -- --mode disko ./btrfs-single-partition.nix --arg device '"/dev/sda"'
{
  device,
  ...
}: {
  disko.devices = {
    disk.single = {
      inherit device;
      type = "disk";
      content = {
        type = "gpt";
        partitions = {
          root = {
            size = "100%";
            content = {
              type = "btrfs";
              subvolumes = {
                "/data" = {
                  mountOptions = ["compress=zstd" "nofail" "rw" "users"];
                  mountpoint = "/mnt/data";
                };
              };
            };
          };
        };
      };
    };
  };
}
