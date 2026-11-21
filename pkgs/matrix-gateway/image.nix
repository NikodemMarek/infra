{
  lib,
  dockerTools,
  matrix-gateway,
}:
dockerTools.buildLayeredImage {
  name = "matrix-gateway";
  compressor = "none";
  config.Entrypoint = [(lib.getExe matrix-gateway)];
  config.Env = ["XDG_CONFIG_HOME=/var/empty"];
}
