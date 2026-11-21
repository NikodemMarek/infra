{
  persist = import ./persist.nix;
  secrets = import ./secrets.nix;
  battery-notifier = import ./battery-notifier.nix;
  flux-sops-age = import ./flux-sops-age.nix;
  eso-bws-token = import ./eso-bws-token.nix;
  agents = import ./agents.nix;
}
