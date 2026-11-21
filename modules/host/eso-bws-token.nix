# Applies the access token External Secrets Operator reads Bitwarden Secrets
# Manager with as secret external-secrets/bitwarden-access-token. Enable it on
# the k3s server only: moving the server role means moving the flag. The token
# is `ESO_BWS_TOKEN` in the secretspec.toml profile `dijkstra`.
{
  lib,
  config,
  ...
}: let
  cfg = config.services.eso-bws-token;
in {
  options.services.eso-bws-token.enable = lib.mkEnableOption "applying the External Secrets Bitwarden access token to the k3s cluster";

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = config.services.k3s.enable && config.services.k3s.role == "server";
        message = "services.eso-bws-token applies the token through the local k3s API, so it must be enabled on the k3s server only (services.k3s.enable = true, services.k3s.role = \"server\").";
      }
    ];

    secrets.eso_bws_token.profile = "dijkstra";

    systemd.services.eso-bws-token = {
      description = "Apply the External Secrets Bitwarden access token to the cluster";
      after = ["k3s.service"];
      requires = ["k3s.service"];
      wantedBy = ["multi-user.target"];
      environment.KUBECONFIG = "/etc/rancher/k3s/k3s.yaml";
      path = [config.services.k3s.package];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # must stay below deploy-rs's activation timeout (240s default), so a stuck
        # start fails the switch instead of deploy-rs timing out first
        TimeoutStartSec = "3min";
      };
      script = ''
        until k3s kubectl get --raw=/readyz > /dev/null 2>&1; do sleep 5; done

        k3s kubectl create namespace external-secrets --dry-run=client -o yaml \
          | k3s kubectl apply --server-side --field-manager=eso-bws-token -f -
        k3s kubectl -n external-secrets create secret generic bitwarden-access-token --from-file=token=${config.secrets.eso_bws_token.path} --dry-run=client -o yaml \
          | k3s kubectl apply --server-side --force-conflicts --field-manager=eso-bws-token -f -
      '';
    };
  };
}
