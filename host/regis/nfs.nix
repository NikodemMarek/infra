{
  services.nfs.server = {
    enable = true;
    exports = ''
      /mnt/models         100.64.0.0/10(rw,sync,no_subtree_check,mp,fsid=8)
      /mnt/projects       100.64.0.0/10(rw,sync,no_subtree_check,mp,fsid=9)
    '';
  };

  systemd.services.nfs-server = {
    after = ["zfs-mount.service" "zfs.target"];
    wants = ["zfs-mount.service"];
    before = ["remote-fs.target"];
  };
}
