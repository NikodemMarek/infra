{
  lib,
  dockerTools,
  matrix-gateway,
}:
dockerTools.buildLayeredImage {
  name = "matrix-gateway";
  compressor = "none";
  config = {
    Entrypoint = [(lib.getExe matrix-gateway)];
    Env = ["XDG_CONFIG_HOME=/var/empty"];
    Labels."org.opencontainers.image.source" = "https://github.com/NikodemMarek/infra";
  };
}
