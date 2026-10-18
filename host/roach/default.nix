{config, ...}: {
  imports = [
    ./hardware-configuration.nix

    ../features
    ../features/optional/systemd-boot.nix
    ../features/optional/tailscale.nix
    ../features/optional/maintenance.nix
    ../features/optional/k3s.nix

    # ./printing.nix
    ./nfs.nix
  ];

  networking = {
    # hostId = "a3a35e1d";
    hostName = "roach";
  };

  time.timeZone = "Europe/Warsaw";

  persist = {
    enable = true;
    deviceService = "dev-nvme0n1p2.device";
    rootPath = "/dev/nvme0n1p2";
    isCrypted = false;
  };

  systemd.network = {
    links = {
      "10-wired-rename" = {
        matchConfig = {
          Type = "ether";
          Kind = "!*";
        };
        linkConfig = {Name = "eth0";};
      };
      "10-wireless-rename" = {
        matchConfig = {
          Type = "wlan";
          Kind = "!*";
        };
        linkConfig = {Name = "wlan0";};
      };
    };
    networks = {
      "10-ethernet" = {
        matchConfig.Type = "ether";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
          IgnoreCarrierLoss = "3s";
        };
        linkConfig.RequiredForOnline = "no";
      };
      "20-wireless-dhcp" = {
        matchConfig.Type = "wlan wwan";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
          IgnoreCarrierLoss = "3s";
        };
      };
    };
  };
  networking.wireless.iwd = {
    enable = true;
    settings = {
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

  services.k3s = {
    role = "agent";
    serverAddr = "https://100.97.10.25:6443";
    extraFlags = toString [
      "--kube-proxy-arg=proxy-mode=nftables"
      "--vpn-auth-file=${config.secrets.k3s_vpn_auth.path}"
      "--node-external-ip=100.109.230.83"
    ];
  };
}
