{config, ...}: {
  nix = {
    settings = {
      experimental-features = ["nix-command" "flakes"];
      extra-substituters = ["http://nix-cache:3902?priority=50"];
      extra-trusted-public-keys = ["nix-cache-1:WFfV/XlLBXZdMcKtaAEB4MSIQlXf6FYa6rqGxxmUcw0="];
      connect-timeout = 5;
      fallback = true;
    };
    gc = {
      automatic = true;
      dates = "weekly";
      options = "--delete-older-than 30d";
    };
    optimise.automatic = true;
  };

  networking.hosts.${config.tailnet.ips.regis} = ["nix-cache"];
}
