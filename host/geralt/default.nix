{pkgs, ...}: {
  imports = [
    ./hardware-configuration.nix
    ./idle-suspend.nix

    ../features
    ../features/optional/docker.nix
    ../features/optional/iwd.nix
    ../features/optional/systemd-boot.nix
    ../features/optional/tailscale-server.nix
    ../features/optional/libvirt.nix
    ../features/optional/wolf.nix
    ../features/optional/maintenance.nix
  ];

  environment.systemPackages = [
    pkgs.ffmpeg
  ];

  networking = {
    hostId = "76cc60bb";
    hostName = "geralt";

    # Wolf (Moonlight) for home clients without tailscale; 5353 is mDNS discovery.
    # Ethernet only: wlan0 is just a fallback for remote debugging.
    firewall.interfaces.eth0 = {
      allowedTCPPorts = [47984 47989 48010];
      allowedUDPPorts = [47999 48100 48200 5353];
    };
  };

  systemd.network = {
    links = {
      "10-wired-rename" = {
        matchConfig = {
          Type = "ether";
          Kind = "!*";
        };
        linkConfig = {
          Name = "eth0";
          WakeOnLan = "magic";
        };
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
      "10-default-dhcp" = {
        matchConfig.Type = "ether wwan";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
          IgnoreCarrierLoss = "3s";
        };
      };
      "10-wireless-dhcp" = {
        matchConfig.Type = "wlan";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
          IgnoreCarrierLoss = "3s";
        };
        dhcpV4Config.RouteMetric = 2048;
        ipv6AcceptRAConfig.RouteMetric = 2048;
      };
    };
  };
  time.timeZone = "Europe/Warsaw";

  persist = {
    enable = true;
    deviceService = "dev-nvme0n1p2.device";
    rootPath = "/dev/nvme0n1p2";
  };
}
