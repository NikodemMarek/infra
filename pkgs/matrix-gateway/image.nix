{
  dockerTools,
  matrix-gateway,
}: let
  matrix-gateway' = matrix-gateway.overridePythonAttrs {
    doCheck = false;
    preBuild = "";
  };
in
  dockerTools.buildLayeredImage {
    name = "matrix-gateway";
    compressor = "none";
    config.Entrypoint = ["${matrix-gateway'}/bin/matrix-gateway"];
    config.Env = ["XDG_CONFIG_HOME=/var/empty"];
  }
