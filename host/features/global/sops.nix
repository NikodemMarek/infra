# Provider for the secrets contract (modules/host/secrets.nix): decrypts
# <host>.yaml from the `secrets` flake input with sops-nix.
# The file is generated from SecretSpec by `secrets-build` (see devenv.nix); the
# `host-switch`, `host-deploy` and `install-remote` scripts pass the directory
# with `--override-input secrets`. It is never committed. Secret names are flat
# top-level keys in it.
# This is the only place that should know secrets come from sops.
{
  lib,
  config,
  inputs,
  ...
}: {
  sops = {
    defaultSopsFile = "${inputs.secrets}/${config.networking.hostName}.yaml";
    defaultSopsFormat = "yaml";

    age = {
      sshKeyPaths = ["/persist/data/etc/ssh/ssh_host_ed25519_key"];
    };

    secrets = lib.mapAttrs (_: s:
      {
        inherit (s) owner group mode neededForUsers;
      }
      // lib.optionalAttrs (!s.neededForUsers) {
        inherit (s) path;
      })
    config.secrets;
  };
}
