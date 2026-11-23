{
  config,
  lib,
  ...
}: let
  host = config.networking.hostName;
  hasPods = config.services.k3s.enable;
  loki = "http://100.97.10.25:3100/loki/api/v1/push";
  victoriaMetrics = "http://100.97.10.25:8428/api/v1/write";
in {
  services.alloy.enable = true;

  environment.etc."alloy/config.alloy".text =
    ''
      prometheus.exporter.unix "node" {
        enable_collectors = ["systemd"]

        filesystem {
          mount_points_exclude = `^/(dev|proc|run/credentials/.+|sys|var/lib/docker/.+)($|/)|^/var/lib/kubelet/.+|^/run/k3s/.+`
        }
      }

      discovery.relabel "node" {
        targets = prometheus.exporter.unix.node.targets

        rule {
          target_label = "job"
          replacement  = "node"
        }

        rule {
          target_label = "instance"
          replacement  = "${host}"
        }

        rule {
          target_label = "host"
          replacement  = "${host}"
        }
      }

      prometheus.scrape "node" {
        targets         = discovery.relabel.node.output
        scrape_interval = "60s"
        forward_to      = [prometheus.remote_write.victoria_metrics.receiver]
      }

      prometheus.remote_write "victoria_metrics" {
        endpoint {
          url = "${victoriaMetrics}"
        }
      }

      loki.relabel "journal" {
        forward_to = []

        rule {
          source_labels = ["__journal__systemd_unit"]
          target_label  = "unit"
        }

        rule {
          source_labels = ["__journal_priority_keyword"]
          target_label  = "level"
        }
      ${lib.optionalString (host == "yenn") (lib.replaceStrings ["\n"] ["\n  "] ''

        rule {
          action        = "drop"
          source_labels = ["__journal__systemd_user_unit"]
          regex         = ".+"
        }''
      + "\n")}}

      loki.source.journal "journal" {
        max_age       = "12h"
        relabel_rules = loki.relabel.journal.rules
        labels        = {
          job  = "journal",
          host = "${host}",
        }
        forward_to    = [loki.write.default.receiver]
      }

      loki.write "default" {
        endpoint {
          url                 = "${loki}"
          max_backoff_retries = 0
        }
      }
    ''
    + lib.optionalString hasPods ''

      local.file_match "pods" {
        path_targets = [{
          __path__ = "/var/log/pods/*/*/*.log",
          job      = "pods",
          host     = "${host}",
        }]
      }

      loki.source.file "pods" {
        targets    = local.file_match.pods.targets
        forward_to = [loki.process.pods.receiver]
      }

      loki.process "pods" {
        forward_to = [loki.write.default.receiver]

        stage.cri {}

        stage.regex {
          source     = "filename"
          expression = `^/var/log/pods/(?P<namespace>[^_/]+)_(?P<pod>[^_/]+)_[^/]+/(?P<container>[^/]+)/[^/]+\.log$`
        }

        stage.labels {
          values = {
            namespace = "",
            pod       = "",
            container = "",
          }
        }

        stage.label_drop {
          values = ["filename"]
        }

        stage.match {
          selector = `{namespace=~"default|jellyfin",container=~"traefik|jellyfin"}`

          stage.replace {
            expression = `(\?[^ "]*)`
            replace    = ""
          }
        }
      }
    '';

  systemd.services.alloy.serviceConfig = {
    AmbientCapabilities = lib.mkIf hasPods ["CAP_DAC_READ_SEARCH"];
    SystemCallFilter = ["~open_by_handle_at"];
    InaccessiblePaths = [
      "-/persist"
      "-/run/secrets"
      "-/run/secrets.d"
      "-/run/secrets-for-users"
      "-/var/lib/rancher"
      "-/var/lib/kubelet"
      "-/etc/rancher"
      "-/root"
      "-/home"
    ];
  };

  persist.generated.directories = [
    {
      directory = "/var/lib/private";
      user = "root";
      group = "root";
      mode = "700";
    }
  ];
}
