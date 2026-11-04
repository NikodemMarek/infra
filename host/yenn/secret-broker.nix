{
  services.secret-broker = {
    enable = true;
    user = "nikodem";
    exportProvider = "bw://?folder=infra/{profile}";
    exportProfiles = builtins.attrNames (builtins.fromTOML (builtins.readFile ../../secretspec.toml)).profiles;
    bwServer = null;
    actions = {};
  };
}
