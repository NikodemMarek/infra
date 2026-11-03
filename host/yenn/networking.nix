{
  imports = [
    ../features/optional/tailscale.nix
    ../features/optional/zerotier.nix
    ./wireguard.nix
  ];

  networking.nameservers = ["10.43.0.53"];

  systemd.network = {
    links = {
      "10-random-mac" = {
        matchConfig = {
          Type = "ether wlan wwan";
          Kind = "!*";
        };
        linkConfig = {MACAddressPolicy = "random";};
      };
    };
    networks = {
      "10-default-dhcp" = {
        matchConfig.Type = "ether wlan wwan";
        networkConfig = {
          DHCP = "yes";
          DNSDefaultRoute = false;
          Domains = ["~vpn.nkmrk.com"];
          IPv6AcceptRA = "yes";
          IgnoreCarrierLoss = "3s";
        };
        dhcpV4Config = {
          UseRoutes = false;
          UseGateway = true;
        };
      };
      "80-forward" = {
        matchConfig.Name = "wlan0";
        networkConfig.IPMasquerade = "both";
      };
    };
    wait-online.enable = false;
  };
  boot.initrd.systemd.network.wait-online.enable = false;

  networking.wireless.iwd = {
    enable = true;
    settings = {
      General = {
        AddressRandomization = "network";
        AddressRandomizationRange = "full";
      };
      Network.EnableIPv6 = true;
      Settings.AutoConnect = true;
    };
  };

  persist.generated.directories = [
    {
      directory = "/var/lib/iwd";
      user = "root";
      group = "root";
    }
  ];
}
