{
  virtualisation.oci-containers = {
    backend = "docker";
    containers.wolf = {
      # Pinned multi-arch index digest, bump deliberately.
      image = "ghcr.io/games-on-whales/wolf:stable@sha256:b0ef253739f9bd49b71eba7e150c955fd05990c0d409e666d89236995351551a";
      networks = ["host"];
      devices = ["/dev/dri" "/dev/uinput" "/dev/uhid"];
      extraOptions = ["--device-cgroup-rule=c 13:* rmw"];
      # Otherwise every launch aborts with "Wayland socket  was not ready"
      # (https://github.com/games-on-whales/wolf/issues/462).
      environment.WOLF_SKIP_WAYLAND_SOCKET_WAIT = "TRUE";
      volumes = [
        # Same path on the host and in the container: Wolf bind-mounts subdirs
        # into the app containers it starts through the host docker socket.
        "/etc/wolf:/etc/wolf:rw"
        "/var/run/docker.sock:/var/run/docker.sock:rw"
        "/dev:/dev:rw"
        "/run/udev:/run/udev:rw"
      ];
    };
  };

  # The first switch starts the new /etc/wolf bind mount and the container in one transaction.
  systemd.services.docker-wolf.unitConfig.RequiresMountsFor = "/etc/wolf";

  # Loaded only on demand otherwise, but the container's --device needs the nodes.
  boot.kernelModules = ["uinput" "uhid"];

  # From https://github.com/games-on-whales/wolf/blob/stable/85-wolf.rules
  # The virtual-device rules keep streamed controllers off the host seat.
  services.udev.extraRules = ''
    KERNEL=="uinput", SUBSYSTEM=="misc", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput", TAG+="uaccess"
    KERNEL=="uhid", MODE="0660", GROUP="input", TAG+="uaccess"
    SUBSYSTEM=="input",  ATTR{name}=="Wolf *virtual*",  MODE="0660", GROUP="input", ENV{ID_SEAT}="seat9", TAG-="uaccess"
    SUBSYSTEMS=="input", ATTRS{name}=="Wolf *virtual*", MODE="0660", GROUP="input", ENV{ID_SEAT}="seat9", TAG-="uaccess"
    KERNEL=="hidraw*",   ATTRS{uevent}=="*HID_NAME=Wolf *virtual*", MODE="0660", GROUP="input", ENV{ID_SEAT}="seat9", TAG-="uaccess"
  '';

  # config.toml (Wolf-owned, holds the paired clients), certs and the per-client
  # app homes, including game libraries.
  persist.data.directories = ["/etc/wolf"];
}
