{
  lib,
  dockerTools,
  runCommand,
  nats-server,
}: let
  conf = runCommand "nats.conf" {} ''
    cp ${./nats.conf} $out
    ${lib.getExe nats-server} -c $out -t
  '';
in
  dockerTools.streamLayeredImage {
    name = "nats-server";
    config.Entrypoint = [(lib.getExe nats-server) "-c" "${conf}"];
  }
