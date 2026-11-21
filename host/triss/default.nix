{
  lib,
  pkgs,
  ...
}: {
  imports = [
    ./hardware-configuration.nix

    ../features
    ../features/optional/systemd-boot.nix
    ../features/optional/tailscale-server.nix
    ../features/optional/maintenance.nix
    ../features/optional/k3s.nix
  ];

  boot.loader.systemd-boot.configurationLimit = lib.mkForce 1;
  boot.kernelModules = ["wireguard"];
  nix.settings = {
    extra-substituters = lib.mkForce [];
    extra-trusted-public-keys = lib.mkForce [];
  };
  networking = {
    hostName = "triss";
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
    };
    networks = {
      "10-default-dhcp" = {
        matchConfig.Name = "eth0";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
        };
      };
    };
  };

  persist = {
    enable = true;
    deviceService = "dev-sda2.device";
    rootPath = "/dev/sda2";
    isCrypted = false;
  };

  services = {
    eso-bws-token.enable = true;

    k3s = {
      role = "server";
      images = [pkgs.shani-image pkgs.matrix-gateway-image];
      extraFlags = [
        "--disable=traefik"
        "--disable-cloud-controller"
      ];
    };
  };
}
