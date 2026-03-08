{
  pkgs,
  lib,
  config,
  inputs,
  ...
}: {
  packages = [pkgs.kubectl pkgs.fluxcd pkgs.sops pkgs.age pkgs.yamlfmt pkgs.nfs-utils pkgs.k9s pkgs.k3s];

  scripts = {
    mksecret.exec = "sops --encrypt --encrypted-regex '^(data|stringData)$' --in-place $1";
  };

  git-hooks.hooks = {
    yamlfmt.enable = true;
  };

  enterShell = ''
    export KUBECONFIG=kubeconfig.yaml
  '';
}
