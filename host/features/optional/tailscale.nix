{config, ...}: {
  services.tailscale.enable = true;

  systemd.services.tailscaled.serviceConfig.Environment = [
    "TS_DEBUG_FIREWALL_MODE=nftables"
  ];

  persist.generated.directories = [
    {
      directory = "/var/lib/tailscale";
      user = "root";
      group = "root";
      mode = "700";
    }
  ];

  networking = {
    firewall = {
      checkReversePath = "loose";
      trustedInterfaces = ["tailscale0"];
    };
    nftables.tables = {
      tailscale-filter = {
        family = "inet";
        content = builtins.replaceStrings ["@tailscalePort@"] [(toString config.services.tailscale.port)] (builtins.readFile ./tailscale-filter.nft);
      };
      tailscale-nat = {
        family = "ip";
        content = builtins.readFile ./tailscale-nat.nft;
      };
    };
  };
}
