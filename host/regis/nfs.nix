{
  services.nfs.server = {
    enable = true;
    exports = ''
      /mnt/models         100.0.0.0/8(rw,sync,fsid=8,insecure,no_root_squash,no_subtree_check) 10.42.0.0/16(rw,sync,fsid=6,insecure,no_root_squash,no_subtree_check)
      /mnt/projects       100.64.0.0/10(rw,sync,no_subtree_check,mp,fsid=9)
    '';
  };

  systemd.services.nfs-server = {
    after = ["zfs-mount.service" "zfs.target"];
    wants = ["zfs-mount.service"];
    before = ["remote-fs.target"];
  };
}
