{config, ...}: {
  secrets.wireguard_private_key.owner = "systemd-network";

  networking.firewall.checkReversePath = "loose";

  security.sudo.extraRules = [
    {
      groups = ["wheel"];
      commands = [
        {
          command = "/run/current-system/sw/bin/networkctl up wg0";
          options = ["NOPASSWD"];
        }
        {
          command = "/run/current-system/sw/bin/networkctl down wg0";
          options = ["NOPASSWD"];
        }
      ];
    }
  ];

  systemd.network = {
    config.networkConfig.ManageForeignRoutingPolicyRules = false;
    netdevs."50-wg0" = {
      netdevConfig = {
        Kind = "wireguard";
        Name = "wg0";
        MTUBytes = 1200;
      };
      wireguardConfig = {
        PrivateKeyFile = config.secrets.wireguard_private_key.path;
        FirewallMark = 51820;
        RouteTable = 51820;
      };
      wireguardPeers = [
        {
          PublicKey = "dBfXS7pfQv88cFNWPeVdgt0qUsYT6v0uG6rCbAhSsVU=";
          Endpoint = "vpn.nkmrk.com:51820";
          AllowedIPs = ["0.0.0.0/0"];
          PersistentKeepalive = 25;
        }
      ];
    };
    networks."50-wg0" = {
      matchConfig.Name = "wg0";
      address = ["10.13.13.130/32"];
      networkConfig = {
        DNS = ["10.43.0.53"];
        Domains = ["~."];
        DNSDefaultRoute = true;
      };
      routes = [
        {
          Destination = "::/0";
          Type = "unreachable";
          Table = 51820;
        }
      ];
      # after tailscale's 5210-5270
      routingPolicyRules = [
        {
          Table = "main";
          SuppressPrefixLength = 0;
          Priority = 5280;
          Family = "both";
        }
        {
          FirewallMark = 51820;
          InvertRule = true;
          Table = 51820;
          Priority = 5290;
          Family = "both";
        }
      ];
    };
  };
}
