{
  lib,
  runCommand,
  dockerTools,
  nats-server,
}: let
  conf = runCommand "nats.conf" {nativeBuildInputs = [nats-server];} ''
    cp ${./nats.conf} $out
    nats-server -c $out -t
  '';
in
  dockerTools.streamLayeredImage {
    name = "nats-server";
    config = {
      Entrypoint = [(lib.getExe nats-server) "-c" "${conf}"];
      Labels."org.opencontainers.image.source" = "https://github.com/NikodemMarek/infra";
    };
  }
