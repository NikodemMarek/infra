{
  persist = import ./persist.nix;
  secrets = import ./secrets.nix;
  battery-notifier = import ./battery-notifier.nix;
  flux-sops-age = import ./flux-sops-age.nix;
  agents = import ./agents.nix;
  secret-broker = import ./secret-broker.nix;
}
