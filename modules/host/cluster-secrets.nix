# Applies the Secret `secrets` of every cluster namespace from the secretspec.toml
# profiles `dijkstra-<ns>` (the whole profile, see `wholeProfile` in secrets.nix).
# Enable it on the k3s server only: moving the server role means moving the flag.
# The Secrets carry `prune: disabled` so Flux never garbage collects them.
{
  pkgs,
  lib,
  config,
  ...
}: let
  cfg = config.services.cluster-secrets;

  prefix = "dijkstra-";
  namespaces =
    map (lib.removePrefix prefix)
    (lib.filter (lib.hasPrefix prefix)
      (lib.attrNames (builtins.fromTOML (builtins.readFile ../../secretspec.toml)).profiles));
  secretName = ns: "dijkstra_${lib.replaceStrings ["-"] ["_"] ns}";
in {
  options.services.cluster-secrets.enable = lib.mkEnableOption "applying the cluster's Kubernetes secrets from the secretspec.toml profiles dijkstra-<ns>";

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.services.k3s.enable && config.services.k3s.role == "server";
        message = "services.cluster-secrets applies the Secrets through the local k3s API, so it must be enabled on the k3s server only (services.k3s.enable = true, services.k3s.role = \"server\").";
      }
    ];

    secrets = lib.listToAttrs (map (ns:
      lib.nameValuePair (secretName ns) {
        profile = "${prefix}${ns}";
        wholeProfile = true;
        restartUnits = ["cluster-secrets.service"];
      })
    namespaces);

    systemd.services.cluster-secrets = {
      description = "Apply the cluster's Kubernetes secrets";
      after = ["k3s.service"];
      requires = ["k3s.service"];
      wantedBy = ["multi-user.target"];
      environment.KUBECONFIG = "/etc/rancher/k3s/k3s.yaml";
      path = [config.services.k3s.package pkgs.jq];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # must stay below deploy-rs's activation timeout (240s default), so a stuck
        # start fails the switch instead of deploy-rs timing out first
        TimeoutStartSec = "3min";
      };
      script = ''
        until k3s kubectl get --raw=/readyz > /dev/null 2>&1; do sleep 5; done
        ${lib.concatMapStringsSep "\n" (ns: ''
            k3s kubectl create namespace ${ns} --dry-run=client -o yaml \
              | k3s kubectl apply --server-side --field-manager=cluster-secrets -f -
            jq --arg ns ${ns} -f ${./cluster-secret.jq} ${config.secrets.${secretName ns}.path} \
              | k3s kubectl apply --server-side --force-conflicts --field-manager=cluster-secrets -f -
          '')
          namespaces}
      '';
    };
  };
}
