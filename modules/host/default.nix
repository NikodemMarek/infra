{
  persist = import ./persist.nix;
  secrets = import ./secrets.nix;
  battery-notifier = import ./battery-notifier.nix;
  cluster-secrets = import ./cluster-secrets.nix;
  agents = import ./agents.nix;
}
