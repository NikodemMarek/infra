{
  pkgs,
  nats-server,
  ...
}:
pkgs.symlinkJoin {
  name = "nats-server";
  paths = [nats-server];
  nativeBuildInputs = [pkgs.makeWrapper];
  meta.mainProgram = "nats-server";
  postBuild = ''
    mkdir -p $out/config/nats-server
    cp ${./nats.conf} $out/config/nats-server/nats.conf

    wrapProgram $out/bin/nats-server \
        --add-flags "-c $out/config/nats-server/nats.conf"

    # a bad config fails the build
    $out/bin/nats-server -t

    mkdir -p $out/lib/systemd/system
    substitute ${./nats.service} $out/lib/systemd/system/nats.service --subst-var out
  '';
}
