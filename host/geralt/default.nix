{pkgs, ...}: {
  imports = [
    ./hardware-configuration.nix

    (import ../features/disko/btrfs-persistence-swapfile.nix {
      device = "nvme0n1";
      swap = 38;
    })

    ../features
    ../features/optional/docker.nix
    ../features/optional/systemd-boot.nix
    ../features/optional/tailscale.nix
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
        matchConfig.Type = "ether wlan wwan";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
          IgnoreCarrierLoss = "3s";
        };
      };
    };
  };
  time.timeZone = "Europe/Warsaw";

  persist = {
    enable = true;
    deviceService = "dev-nvme0n1p2.device";
    rootPath = "/dev/nvme0n1p2";
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
}
