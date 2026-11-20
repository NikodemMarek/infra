{
  persist = import ./persist.nix;
  secrets = import ./secrets.nix;
  battery-notifier = import ./battery-notifier.nix;
  flux-sops-age = import ./flux-sops-age.nix;
  cluster-secrets = import ./cluster-secrets.nix;
  agents = import ./agents.nix;
}
