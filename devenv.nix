{
  pkgs,
  lib,
  config,
  inputs,
  ...
}: let
  # Sourced by the scripts below
  common = pkgs.writeText "common.sh" ''
    set -euo pipefail
    # also fail inside $(...), so a failing step in a function aborts the script
    shopt -s inherit_errexit

    # cleanups run in order on exit, a plain `trap` would replace earlier ones
    _cleanups=()
    trap 'for c in "''${_cleanups[@]}"; do eval "$c"; done' EXIT
    on_exit() { _cleanups+=("$1"); }

    # [host...] builds encrypted secrets for the hosts into $secrets
    build_secrets() {
      secrets=$(mktemp -d "''${XDG_RUNTIME_DIR:-/tmp}/secrets.XXXXXX")
      on_exit "rm -rf '$secrets'"
      secrets-build "$secrets" "$@"
    }
  '';
in {
  packages = [
    inputs.deploy-rs.packages.${pkgs.stdenv.hostPlatform.system}.deploy-rs
    pkgs.ssh-to-age
    pkgs.sops
    pkgs.nixos-anywhere
    pkgs.disko
    pkgs.nixos-generators
    pkgs.jq

    pkgs.kubectl
    pkgs.fluxcd
    pkgs.velero
    pkgs.yamlfmt
    pkgs.k9s
    pkgs.k3s
    pkgs.pv-migrate
    pkgs.wireguard-tools
    pkgs.qrencode
    pkgs.nix
  ];

  languages.rust.enable = true;

  treefmt = {
    enable = true;
    config = {
      programs = {
        alejandra.enable = true;
        ruff-format.enable = true;
        rustfmt = {
          enable = true;
          edition = "2021";
        };
        shfmt.enable = true;
        terraform = {
          enable = true;
          package = pkgs.opentofu;
        };
        taplo.enable = true;
        yamlfmt = {
          enable = true;
          settings.formatter.retain_line_breaks_single = true;
        };
        prettier = {
          enable = true;
          includes = ["*.json" "*.css"];
        };
        fish_indent.enable = true;
        stylua.enable = true;
      };
      settings.global.excludes = [
        ".sops.yaml"
        "*.lock"
        "*.pub"
        "*.cred"
        "*.png"
        "clusters/*/flux-system/gotk-*.yaml"
        "clusters/*/monitoring/grafana/dashboards/*.json"
      ];
    };
  };

  tasks."devenv:treefmt:run".status = "true";
  tasks."devenv:git-hooks:install".status = "true";

  git-hooks.hooks = {
    treefmt.enable = true;
    statix.enable = true;
    deadnix.enable = true;
    ruff.enable = true;
    shellcheck.enable = true;
    clippy = {
      enable = true;
      entry = lib.mkForce "${pkgs.writeShellScript "clippy-crates" ''
        for manifest in pkgs/*/Cargo.toml; do
          cargo clippy --manifest-path "$manifest" --all-targets -- -D warnings || exit 1
        done
      ''}";
      pass_filenames = false;
    };
  };

  scripts = {
    secrets-build = {
      exec = ''
        exec nix run "$DEVENV_ROOT#secrets-build" -- "$@"
      '';
      description = "[outdir host...] build encrypted sops files from Bitwarden Secrets Manager (needs BWS_ACCESS_TOKEN)";
    };
    tofu = {
      exec = ''
        cd "$DEVENV_ROOT/tofu"
        exec nix run "$DEVENV_ROOT#infra-tofu" -- "$@"
      '';
      description = "[tofu args...] OpenTofu for the external infrastructure, secrets from Bitwarden Secrets Manager (needs BWS_ACCESS_TOKEN)";
    };
    host-switch = {
      exec = ''
        source ${common}

        host=$(hostname)
        build_secrets "$host"

        sudo nixos-rebuild "''${@:-switch}" --flake ".#$host" --override-input secrets "path:$secrets"
      '';
      description = "[nixos-rebuild action] rebuild this host (default: switch)";
    };
    host-deploy = {
      exec = ''
        source ${common}

        if [ "$#" -lt 1 ]; then
          echo "usage: host-deploy node..."
          exit 1
        fi

        build_secrets "$@"

        known_hosts=$(mktemp)
        on_exit "rm -f '$known_hosts'"
        nix eval --raw .#knownHosts > "$known_hosts"

        targets=()
        for n in "$@"; do
          targets+=(".#$n")
        done

        # deploy-rs's flake check evaluates every host, but $secrets only holds the requested ones
        deploy --skip-checks \
          --ssh-opts "-o UserKnownHostsFile=$known_hosts -o StrictHostKeyChecking=yes" \
          --targets "''${targets[@]}" -- --override-input secrets "path:$secrets"
      '';
      description = "[node...] deploy config to remote hosts";
    };
    install-remote = {
      exec = ''
        source ${common}

        if [ "$#" -lt 2 ]; then
          echo "usage: install-remote host user@ip"
          exit 1
        fi
        host=$1
        target=$2

        if [ ! -f "host/$host/ssh_host_ed25519_key" ]; then
          echo "missing host key host/$host/ssh_host_ed25519_key"
          exit 1
        fi

        # [attr] builds config.system.build.<attr> of the host with its secrets;
        # nixos-anywhere has no --override-input, so it gets the store paths instead
        build() {
          nix build --impure --no-link --print-out-paths \
            ".#nixosConfigurations.$host.config.system.build.$1" \
            --override-input secrets "path:$secrets"
        }

        build_secrets "$host"

        # files copied onto the new disk: the host key, which is also the host's sops identity
        extra=$(mktemp -d)
        on_exit "rm -rf '$extra'"
        install -d -m755 "$extra/persist/data/etc/ssh"
        install -m600 "host/$host/ssh_host_ed25519_key" "$extra/persist/data/etc/ssh/ssh_host_ed25519_key"

        # zfs hosts (regis' tank) read their disk key from /tmp/disk.key during the
        # install and from /persist afterwards; no other layout uses it
        zpool=$(nix eval --impure --raw ".#nixosConfigurations.$host.config.disko.devices.zpool" \
          --override-input secrets "path:$secrets" \
          --apply 'z: if z != {} then "1" else ""')
        disk_key=()
        if [ -n "$zpool" ]; then
          if [ ! -f "host/$host/disk.key" ]; then
            echo "missing disk key host/$host/disk.key"
            exit 1
          fi
          install -d -m755 "$extra/persist/data/etc/zfs"
          install -m600 "host/$host/disk.key" "$extra/persist/data/etc/zfs/disk.key"
          disk_key=(--disk-encryption-keys /tmp/disk.key "host/$host/disk.key")
        fi

        # assigned first, set -e ignores a failing $(...) inside the arguments below
        disko=$(build diskoScript)
        toplevel=$(build toplevel)

        nixos-anywhere \
          "''${disk_key[@]}" \
          --extra-files "$extra" \
          --store-paths "$disko" "$toplevel" \
          "$target"
      '';
      description = "[host user@ip] install on remote host";
    };
    mkiso = {
      exec = "nixos-generate --format iso --configuration ./host/alp/default.nix -o result";
      description = "generate iso file";
    };
    writeiso = {
      exec = ''
        ISO=$(ls result/iso/*.iso)
        sudo dd if=$ISO of=$1 status=progress && sync
      '';
      description = "[/dev/XXX] write iso file to device";
    };
  };

  enterShell = ''
    export KUBECONFIG="$(jj workspace root --name default --ignore-working-copy 2>/dev/null || echo "$DEVENV_ROOT")/kubeconfig.yaml"

    printf "\e[33m
    ${lib.concatStringsSep "\n" (lib.mapAttrsToList (name: script: "\\e[1m${name}\\e[0m\\e[33m \t\t -> ${script.description}") config.scripts)}
    \e[0m"
  '';
}
