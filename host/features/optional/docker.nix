{
  virtualisation.docker = {
    enable = true;
    storageDriver = "btrfs";
    daemon.settings = {
      dns = ["1.1.1.1" "8.8.8.8"];
    };
  };

  networking.firewall.trustedInterfaces = ["docker0" "br-+"];

  systemd.network.networks."05-docker-veth" = {
    matchConfig.Name = "veth* docker* br-*";
    linkConfig.Unmanaged = true;
  };

  persist.generated.directories = [
    "/var/lib/docker"
  ];
}
