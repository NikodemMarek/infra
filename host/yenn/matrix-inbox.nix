{
  config,
  lib,
  pkgs,
  ...
}: let
  handlers = pkgs.writeText "matrix-inbox-handlers" ''
    events.claude.permission_request ${pkgs.matrix-inbox}/bin/matrix-inbox-post
    events.claude.question ${pkgs.matrix-inbox}/bin/matrix-inbox-post
    matrix.reply.${config.networking.hostName} ${pkgs.matrix-inbox}/bin/matrix-inbox-answer
  '';
in {
  systemd.user.services.matrix-inbox = {
    description = "Forward agent-inbox questions and permission prompts to the matrix-gateway";
    wantedBy = ["default.target"];
    unitConfig.ConditionUser = "nikodem";
    environment.MATRIX_GATEWAY_KEY = "Kt+wbhrOGYkZ0bc/w0Ed4XTYPIM5oVSypAAthCsaKBk=";
    serviceConfig = {
      ExecStart = lib.getExe (pkgs.event-handlers.override {inherit handlers;});
      Restart = "always";
      RestartSec = 2;
      KillMode = "process";
    };
  };
}
