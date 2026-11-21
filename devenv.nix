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

    # unlock once, nested scripts inherit BW_SESSION and skip this
    bw_unlock() {
      if [ -z "''${BW_SESSION:-}" ]; then
        BW_SESSION=$(bw unlock --raw)
        on_exit 'bw lock > /dev/null'
      fi
      export BW_SESSION
    }

    # [anchor] the age recipient behind &anchor in .sops.yaml
    sops_recipient() { sed -n "s/.*&$1 \(age1[0-9a-z]*\).*/\1/p" .sops.yaml; }

    # [host...] builds encrypted secrets for the hosts into $secrets
    build_secrets() {
      secrets=$(mktemp -d "''${XDG_RUNTIME_DIR:-/tmp}/secrets.XXXXXX")
      on_exit "rm -rf '$secrets'"
      secrets-build "$secrets" "$@"
    }
  '';

  selectSecrets = ./pkgs/secrets-build/select-secrets.jq;

  # Input: the secrets of one namespace (output of selectSecrets). $ns: the namespace.
  # Output: the Secret `secrets` of that namespace.
  renderSecret = pkgs.writeText "render-secret.jq" ''
    {
      apiVersion: "v1",
      kind: "Secret",
      metadata: {name: "secrets", namespace: $ns},
      type: "Opaque",
      stringData: .
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
    pkgs.secretspec
    pkgs.jq
    pkgs.yq-go

    pkgs.kubectl
    pkgs.fluxcd
    pkgs.velero
    pkgs.age
    pkgs.yamlfmt
    pkgs.nfs-utils
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
        "*.enc.yaml"
        ".sops.yaml"
        "*.lock"
        "*.pub"
        "*.cred"
        "*.png"
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
    cluster-secrets-build = {
      exec = ''
        source ${common}

        check=false
        case "''${1:-}" in
          --check) check=true ;;
          "") ;;
          *)
            echo "usage: cluster-secrets-build [--check]"
            exit 1
            ;;
        esac

        dir=clusters/dijkstra/secrets
        tmp=$dir/.build.tmp
        on_exit "rm -f '$tmp'"
        dirty=false

        # [name list] whether the newline separated list has the name
        listed() { printf '%s\n' "$2" | grep -Fx -- "$1" > /dev/null; }

        # [file status] one line per file, never any values
        report() { printf '%-9s %s\n' "$2" "$1"; }

        bw_unlock
        bw sync > /dev/null

        # each profile dijkstra-<ns> is the Secret `secrets` of namespace <ns>, made of every
        # key it declares; this is {"<ns>": [lowercase key...]}, names must be [a-z0-9_]+ and unique
        # assigned first, set -e ignores a failing $(...) inside the arguments below
        declared=$(nix eval --impure --json --expr '
          let
            profiles = (builtins.fromTOML (builtins.readFile ./secretspec.toml)).profiles;
          in
            builtins.mapAttrs (_: builtins.attrNames) profiles
        ' | jq -c '
          with_entries(select(.key | startswith("dijkstra-")) | .key |= ltrimstr("dijkstra-") | .value |= map(ascii_downcase))
          | with_entries(
            if .value == [] then error("\(.key): no secrets in the profile")
            elif [.value[] | select(test("^[a-z0-9_]+$") | not)] != [] then error("\(.key): keys must match ^[a-z0-9_]+$ (lowercased)")
            elif (.value | length) != (.value | unique | length) then error("\(.key): keys collide when lowercased")
            else . end
          )
        ')
        namespaces=$(printf '%s' "$declared" | jq -r 'keys[]')

        # a Secret for a namespace the cluster does not create would break the whole Flux apply
        manifests=$(grep -rlx --include='*.yaml' 'kind: Namespace' clusters/dijkstra || true)
        nsmanifests=$(printf '%s\n' "$manifests" | xargs -r yq eval-all 'select(.kind == "Namespace") | .metadata.name')

        # the cluster key decrypts the existing files, it must be the one behind &dijkstra_cluster
        key=$(secretspec get --profile dijkstra FLUX_SOPS_AGE_KEY)
        have=$(printf '%s\n' "$key" | age-keygen -y)
        want=$(sops_recipient dijkstra_cluster)
        if [ "$have" != "$want" ]; then
          echo "FLUX_SOPS_AGE_KEY is $have, but .sops.yaml has ''${want:-nothing}" >&2
          exit 1
        fi

        # plaintext only ever lives in shell variables and pipes (printf is a builtin, so
        # never argv), the key reaches sops through its environment; $(...) strips trailing
        # newlines, harmless for a whole json document as the values are escaped inside it
        for ns in $namespaces; do
          file=$dir/$ns.enc.yaml
          if ! listed "$ns" "$nsmanifests"; then
            echo "$ns: no kind: Namespace manifest under clusters/dijkstra" >&2
            exit 1
          fi

          names=$(printf '%s' "$declared" | jq -c --arg ns "$ns" '.[$ns]')
          secret=$(secretspec export --profile "dijkstra-$ns" --format json | jq --argjson names "$names" -f ${selectSecrets} | jq --arg ns "$ns" -f ${renderSecret})
          wanted=$(printf '%s' "$secret" | jq -S .)

          status=new
          if [ -f "$file" ]; then
            # the recipients are plaintext metadata; a file for another key (rotation) is rewritten
            recipients=$(yq '.sops.age[].recipient' "$file")
            # a failing decrypt must abort, not count as a change
            if ! current=$(SOPS_AGE_KEY="$key" sops decrypt --output-type json "$file" | jq -S '{apiVersion, kind, metadata: {name: .metadata.name, namespace: .metadata.namespace}, type, stringData}'); then
              echo "$file: cannot be decrypted with FLUX_SOPS_AGE_KEY, it is probably encrypted to an old key: delete clusters/dijkstra/secrets/*.enc.yaml and rerun" >&2
              exit 1
            fi
            if [ "$current" = "$wanted" ] && [ "$recipients" = "$want" ]; then status=unchanged; else status=changed; fi
          fi
          report "$file" "$status"
          if [ "$status" = unchanged ]; then continue; fi

          dirty=true
          if ! $check; then
            mkdir -p "$dir"
            printf '%s' "$secret" | sops encrypt --input-type json --output-type yaml \
              --filename-override "$file" /dev/stdin > "$tmp"
            mv "$tmp" "$file"
          fi
        done

        # files of namespaces that lost their profile
        for file in "$dir"/*.enc.yaml; do
          if [ ! -e "$file" ] || listed "$(basename "$file" .enc.yaml)" "$namespaces"; then continue; fi
          report "$file" stale
          dirty=true
          if ! $check; then rm "$file"; fi
        done

        if $check && $dirty; then exit 1; fi
      '';
      description = "[--check] build the cluster's Kubernetes secrets from bitwarden";
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
