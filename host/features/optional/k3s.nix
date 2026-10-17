{
  config,
  pkgs,
  ...
}: {
  secrets = {
    k3s_token = {};
    k3s_vpn_auth = {};
  };

  environment.systemPackages = [
    pkgs.nfs-utils
  ];

  systemd.tmpfiles.rules = [
    "d /run/flannel 0755 root root -"
  ];

  services.k3s = {
    enable = true;
    tokenFile = config.secrets.k3s_token.path;
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
