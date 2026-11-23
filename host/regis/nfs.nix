{config, ...}: {
  services.nfs.server = {
    enable = true;
    exports = ''
      /mnt/photos         ${config.tailnet.ips.yenn}(ro,no_root_squash,no_subtree_check,mp,fsid=10)
    '';
  };

  systemd.services.nfs-server = {
    after = ["zfs-mount.service" "zfs.target"];
    wants = ["zfs-mount.service"];
    before = ["remote-fs.target"];
  };
}
