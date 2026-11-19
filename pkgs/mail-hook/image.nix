{
  lib,
  dockerTools,
  buildEnv,
  writeText,
  mail-hook,
  mail-hook-handler,
  event-handlers,
}: let
  mail-hook' = mail-hook.overridePythonAttrs {
    doCheck = false;
    preBuild = "";
  };
  handlers = event-handlers.override {
    handlers = writeText "mail-hook-handlers" ''
      mail.received ${lib.getExe mail-hook-handler}
    '';
  };
in
  dockerTools.buildLayeredImage {
    name = "mail-hook";
    compressor = "none";
    contents = buildEnv {
      name = "mail-hook-root";
      paths = [mail-hook' handlers];
      pathsToLink = ["/bin"];
    };
    config.Env = [
      "PATH=/bin"
      "XDG_CONFIG_HOME=/var/empty"
    ];
  }
