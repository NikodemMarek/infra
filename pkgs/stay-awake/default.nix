{
  writeShellApplication,
  libnotify,
}:
# systemctl/systemd-run/systemd-inhibit come from the host: on non-NixOS hosts
# a nix systemd client may not match the running manager.
writeShellApplication {
  name = "stay-awake";
  runtimeInputs = [libnotify];
  text = builtins.readFile ./stay-awake.sh;
}
