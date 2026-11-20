{config, ...}: {
  imports = [./tailscale.nix];

  secrets.tailscale_auth_key.profile = "tailscale";

  services.tailscale = {
    authKeyFile = config.secrets.tailscale_auth_key.path;
    authKeyParameters = {
      ephemeral = false;
      preauthorized = true;
    };
    extraUpFlags = ["--advertise-tags=tag:server"];
  };
}
