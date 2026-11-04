{
  config,
  lib,
  pkgs,
  ...
}: let
  ips = {
    triss = "100.97.10.25";
    regis = "100.100.215.91";
    roach = "100.109.230.83";
  };
  ip = ips.${config.networking.hostName};
  isServer = config.services.k3s.role == "server";
in {
  secrets.k3s_token = {};

  environment.systemPackages = [
    pkgs.nfs-utils
  ];

  systemd.tmpfiles.rules = [
    "d /run/flannel 0755 root root -"
  ];

  services.k3s = {
    enable = true;
    tokenFile = config.secrets.k3s_token.path;
    serverAddr =
      if isServer
      then ""
      else "https://${ips.triss}:6443";
    extraFlags =
      [
        "--kube-proxy-arg=proxy-mode=nftables"
        "--vpn-auth=name=tailscale,joinKey=none"
        "--node-external-ip=${ip}"
        "--node-ip=${ip}"
        "--kubelet-arg=node-ip=${ip}"
        "--flannel-iface=tailscale0"
      ]
      ++ lib.optional (!isServer) "--disable-apiserver-lb";
  };

  systemd.services.k3s = {
    after = ["tailscaled.service"];
    wants = ["tailscaled.service"];
    path = [pkgs.tailscale];
  };

  persist.generated.directories = [
    {
      directory = "/etc/rancher/node";
      user = "root";
      group = "root";
      mode = "755";
    }
    {
      directory = "/var/lib/rancher/k3s";
      user = "root";
      group = "root";
      mode = "755";
    }
    {
      directory = "/var/lib/kubelet";
      user = "root";
      group = "root";
      mode = "755";
    }
    {
      directory = "/var/openebs/local";
      user = "root";
      group = "root";
      mode = "755";
    }
  ];

  networking.firewall = {
    allowedTCPPorts = [6443];
    allowedUDPPorts = [53];

    checkReversePath = "loose";

    trustedInterfaces = ["cni0"];
  };

  systemd.network.networks."05-k3s-interfaces" = {
    matchConfig.Name = "veth*";
    linkConfig.Unmanaged = true;
  };
}
