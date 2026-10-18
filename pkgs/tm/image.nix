{
  dockerTools,
  tm,
  tzdata,
}: let
  tm' = tm.overrideAttrs {doCheck = false;}; # no test run under qemu in triss's closure
in
  dockerTools.buildLayeredImage {
    name = "tm";
    contents = [tzdata]; # /share/zoneinfo for TZ=Europe/Warsaw; chrono's Local ignores TZDIR
    compressor = "none";
    config = {
      Entrypoint = ["${tm'}/bin/tm"];
      Cmd = ["serve" "--listen" "0.0.0.0:8080"];
      ExposedPorts."8080/tcp" = {};
    };
  }
