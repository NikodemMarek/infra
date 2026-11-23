{lib, ...}: {
  options.tailnet.ips = lib.mkOption {
    type = lib.types.attrsOf lib.types.str;
    readOnly = true;
    default = {
      regis = "100.100.215.91";
      roach = "100.109.230.83";
      triss = "100.97.10.25";
      yenn = "100.123.128.10";
    };
  };

  config = {
    services.resolved.enable = true;
    systemd.network.enable = true;

    networking = {
      useNetworkd = true;
      useDHCP = false;
      enableIPv6 = true;

      firewall.enable = true;
      nftables.enable = true;
    };

    boot.kernel.sysctl = {
      "net.ipv4.ip_forward" = 1;
      "net.ipv6.conf.all.forwarding" = 1;
      "net.ipv4.conf.all.rp_filter" = 2;
    };
  };
}
