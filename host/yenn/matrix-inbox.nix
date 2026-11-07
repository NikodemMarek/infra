{
  config,
  lib,
  pkgs,
  ...
}: {
  secrets.matrix_inbox_token = {
    owner = "nikodem";
    group = "users";
  };

  systemd.user.services.matrix-inbox = {
    description = "Mirror agent-inbox questions and permission prompts into Matrix";
    wantedBy = ["default.target"];
    unitConfig.ConditionUser = "nikodem";
    environment = {
      MATRIX_HOMESERVER = "https://matrix.nkmrk.com";
      MATRIX_OWNER = "@nikodem:nkmrk.com";
      MATRIX_TOKEN_FILE = config.secrets.matrix_inbox_token.path;
    };
    serviceConfig = {
      ExecStart = lib.getExe pkgs.matrix-inbox;
      Restart = "always";
      RestartSec = 30;
    };
  };
}
