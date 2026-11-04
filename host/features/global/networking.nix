{
  services.resolved.enable = true;
  systemd.network.enable = true;

  networking = {
    useNetworkd = true;
    useDHCP = false;
    enableIPv6 = true;

    firewall.enable = true;
    nftables = {
      enable = true;
      # A reload replaces only the tables NixOS declares; a flush would also wipe docker, libvirt, k3s and tailscale rules.
      # So add rules through networking.nftables.tables, never .ruleset (it isn't deleted on reload and turns flushing back on).
      flushRuleset = false;
    };
  };

  boot.kernel.sysctl = {
    "net.ipv4.ip_forward" = 1;
    "net.ipv6.conf.all.forwarding" = 1;
    "net.ipv4.conf.all.rp_filter" = 2;
  };
}
