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
    matrix.command.${config.networking.hostName} ${pkgs.matrix-inbox}/bin/matrix-inbox-command
  '';
in {
  systemd.user.services.matrix-inbox = {
    description = "Forward agent-inbox questions and permission prompts to the matrix-gateway";
    wantedBy = ["default.target"];
    unitConfig.ConditionUser = "nikodem";
    environment = {
      PATH = lib.mkForce config.systemd.user.services.event-handlers.environment.PATH;
      inherit (config.systemd.user.services.event-handlers.environment) SSH_AUTH_SOCK;
    };
    serviceConfig = {
      ExecStart = lib.getExe (pkgs.event-handlers.override {inherit handlers;});
      Restart = "always";
      RestartSec = 2;
      KillMode = "process";
    };
  };
}
