{
  config,
  lib,
  pkgs,
  ...
}: let
  handlers = pkgs.writeText "matrix-inbox-handlers" ''
    matrix.command.${config.networking.hostName} ${pkgs.matrix-inbox}/bin/matrix-inbox-command
  '';
in {
  systemd.user.services.matrix-inbox = {
    description = "Run signed commands from the matrix-gateway";
    wantedBy = ["default.target"];
    unitConfig = {
      ConditionUser = "nikodem";
      StartLimitIntervalSec = 0;
    };
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
