{
  config,
  pkgs,
  ...
}: {
  imports = [
    ./hardware-configuration.nix

    ../features
    ../features/optional/systemd-boot.nix
    ../features/optional/tailscale.nix
    ../features/optional/maintenance.nix
    ../features/optional/k3s.nix

    ./nfs.nix
    ./garage.nix
  ];

  networking.hostName = "regis";

  environment.systemPackages = [pkgs.wakeonlan];

  time.timeZone = "Europe/Warsaw";

  persist = {
    enable = true;
    deviceService = "dev-nvme0n1p2.device";
    rootPath = "/dev/nvme0n1p2";
    isCrypted = true;
    isZfs = true;
  };

  systemd.network = {
    links = {
      "10-rename-enp2s0" = {
        matchConfig.MACAddress = "6c:bf:b5:05:77:88";
        linkConfig = {Name = "eth0";};
      };
      "10-rename-enp3s0" = {
        matchConfig.MACAddress = "6c:bf:b5:05:77:89";
        linkConfig = {Name = "eth1";};
      };
    };
    networks = {
      "10-ethernet" = {
        matchConfig.Type = "ether";
        networkConfig = {
          DHCP = "yes";
          IPv6AcceptRA = "yes";
        };
        linkConfig.RequiredForOnline = "no";
      };
    };
    wait-online.enable = false;
  };

  services.k3s = {
    role = "agent";
    serverAddr = "https://100.97.10.25:6443";
    extraFlags = toString [
      "--kube-proxy-arg=proxy-mode=nftables"
      "--vpn-auth-file=${config.secrets.k3s_vpn_auth.path}"
      "--node-external-ip=100.100.215.91"
      "--node-ip=100.100.215.91"
      "--kubelet-arg=node-ip=100.100.215.91"
      "--bind-address=100.100.215.91"
      "--flannel-iface=tailscale0"
      "--disable-apiserver-lb"
    ];
  };
}
