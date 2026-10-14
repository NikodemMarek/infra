{
  pkgs,
  lib,
  config,
  inputs,
  ...
}: let
  # Runs a command and answers deploy-rs's interactive sudo prompts with
  # SUDO_PASSWORD_<host>, asking on the terminal for hosts without one
  sudoExpect = pkgs.writeText "deploy-sudo.exp" ''
    set timeout -1
    foreach name [array names env SUDO_PASSWORD_*] {
      set password([string range $name 14 end]) $env($name)
      unset env($name)
    }

    spawn -noecho {*}$argv
    expect {
      -re {\(sudo for ([^)]+)\) Password: } {
        set host $expect_out(1,string)
        if {[info exists password($host)]} {
          send -- "$password($host)\r"
        } else {
          send_user "(sudo for $host) Password: "
          stty -echo
          expect_user -re "(.*)\n"
          stty echo
          send_user "\n"
          send -- "$expect_out(1,string)\r"
        }
        exp_continue
      }
      eof
    }

    lassign [wait] pid spawn_id os_error status
    exit $status
  '';

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

    # [host...] builds encrypted secrets for the hosts into $secrets
    build_secrets() {
      secrets=$(mktemp -d "''${XDG_RUNTIME_DIR:-/tmp}/secrets.XXXXXX")
      on_exit "rm -rf '$secrets'"
      secrets-build "$secrets" "$@"
    }

    # [host user] login password from bitwarden, fails if there is none
    user_password() { secretspec get --profile "$1" "USERS_''${2^^}_PASSWORD" 2> /dev/null; }

    # [file] key type and body of a public key (from stdin without a file), without the comment
    pubkey() { cut -d' ' -f1-2 "$@"; }

    # [anchor] the age recipient behind &anchor in .sops.yaml
    sops_recipient() { sed -n "s/.*&$1 \(age1[0-9a-z]*\).*/\1/p" .sops.yaml; }
  '';

  # Input: `secretspec export` of one profile. $names: the secrets taken from it.
  # Output: exactly those secrets, with lowercase names; fails if one is missing or empty.
  selectSecrets = pkgs.writeText "select-secrets.jq" ''
    with_entries(.key |= ascii_downcase)
    | ($names - keys) as $missing
    | if $missing != [] then error("missing in bitwarden: \($missing)") end
    | with_entries(select(.key | IN($names[])))
    | [to_entries[] | select(.value == null or .value == "") | .key] as $empty
    | if $empty != [] then error("empty in bitwarden: \($empty)") end
  '';

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
    pkgs.mkpasswd
    pkgs.expect

    pkgs.kubectl
    pkgs.fluxcd
    pkgs.velero
    pkgs.age
    pkgs.yamlfmt
    pkgs.nfs-utils
    pkgs.k9s
    pkgs.k3s
    pkgs.pv-migrate
  ];

  scripts = {
    secrets-build = {
      exec = ''
        source ${common}

        if [ "$#" -lt 2 ]; then
          echo "usage: secrets-build outdir host..."
          exit 1
        fi

        # [json key] value of one secret
        secret() { printf '%s' "$1" | jq -r --arg k "$2" '.[$k]'; }

        # [host] the host's secrets from bitwarden as json, exactly the ones its config declares,
        # each exported from the profile that holds it
        fetch_secrets() {
          local profile_of profiles p names part parts=""
          # assigned first, set -e ignores a failing $(...) inside the arguments below
          profile_of=$(nix eval --json ".#nixosConfigurations.$1.config.secrets" --apply 'builtins.mapAttrs (_: s: s.profile)')
          profiles=$(printf '%s' "$profile_of" | jq -r '[.[]] | unique[]')
          for p in $profiles; do
            names=$(printf '%s' "$profile_of" | jq -c --arg p "$p" '[to_entries[] | select(.value == $p) | .key]')
            part=$(secretspec export --profile "$p" --format json | jq --argjson names "$names" -f ${selectSecrets})
            parts+=$part$'\n'
          done
          printf '%s' "$parts" | jq -s 'add // {}'
        }

        # [host key] repo path of the public half of an ssh private key secret
        pub_file() {
          case $2 in
            host_ssh_ed25519_priv) echo "host/$1/ssh_host_ed25519_key.pub" ;;
            users_*_ssh_id_ed25519)
              local user=''${2#users_}
              echo "host/$1/user_''${user%_ssh_id_ed25519}_ssh_id_ed25519.pub"
              ;;
          esac
        }

        # [host json] ssh private keys must match their public halves in the repo,
        # a rerolled key must come with an updated .pub
        check_ssh_keys() {
          local key file
          for key in $(printf '%s' "$2" | jq -r 'keys[] | select(. == "host_ssh_ed25519_priv" or test("^users_.+_ssh_id_ed25519$"))'); do
            file=$(pub_file "$1" "$key")
            if [ "$(secret "$2" "$key" | ssh-keygen -y -f /dev/stdin | pubkey)" != "$(pubkey "$file" 2> /dev/null || true)" ]; then
              echo "$1: $key in bitwarden does not match $file" >&2
              exit 1
            fi
          done
        }

        # [host] the host key is also the host's sops identity, its age key must be
        # the one behind the &host anchor in .sops.yaml
        check_sops_identity() {
          local have want
          have=$(ssh-to-age < "host/$1/ssh_host_ed25519_key.pub")
          want=$(sops_recipient "$1")
          if [ "$have" != "$want" ]; then
            echo "$1: host key is $have, but .sops.yaml has ''${want:-nothing}" >&2
            exit 1
          fi
        }

        # [host json] the Flux key must be the identity behind the &dijkstra_cluster anchor in
        # .sops.yaml, a wrong or rotated key is caught here instead of on the host
        check_flux_key() {
          local have want
          if [ "$(printf '%s' "$2" | jq 'has("flux_sops_age_key")')" = false ]; then
            return
          fi
          have=$(secret "$2" flux_sops_age_key | age-keygen -y)
          want=$(sops_recipient dijkstra_cluster)
          if [ "$have" != "$want" ]; then
            echo "$1: flux_sops_age_key in bitwarden is $have, but .sops.yaml has ''${want:-nothing}" >&2
            exit 1
          fi
        }

        # [json] login passwords (USERS_*_PASSWORD) are stored in plain text, hashedPasswordFile needs a hash
        hash_passwords() {
          local json=$1 key hash
          for key in $(printf '%s' "$json" | jq -r 'keys[] | select(test("^users_.+_password$"))'); do
            hash=$(secret "$json" "$key" | mkpasswd -s)
            json=$(printf '%s' "$json" | HASH=$hash jq --arg k "$key" '.[$k] = env.HASH')
          done
          printf '%s' "$json"
        }

        # [host json] encrypts with the creation rule .sops.yaml has for the host
        encrypt() {
          printf '%s' "$2" | sops encrypt --input-type json --output-type yaml \
            --filename-override "host/$1/secrets.yaml" /dev/stdin
        }

        outdir=$1
        shift
        mkdir -p "$outdir"
        chmod 700 "$outdir"

        bw_unlock
        bw sync > /dev/null

        # plaintext only ever lives in memory and pipes, never touches the disk
        for h in "$@"; do
          json=$(fetch_secrets "$h")
          check_ssh_keys "$h" "$json"
          check_sops_identity "$h"
          check_flux_key "$h" "$json"
          json=$(hash_passwords "$json")
          encrypt "$h" "$json" > "$outdir/.$h.yaml.tmp"
          mv "$outdir/.$h.yaml.tmp" "$outdir/$h.yaml"
        done
      '';
      description = "[outdir host...] build encrypted sops files from bitwarden";
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

        # each profile dijkstra-<ns> is the Secret `secrets` of namespace <ns>, made of
        # exactly the keys it declares (not the ones that only feed a `composed` key, as for
        # hosts); this is {"<ns>": [lowercase key...]}, names must be [a-z0-9_]+ and unique
        # assigned first, set -e ignores a failing $(...) inside the arguments below
        declared=$(nix eval --impure --json --expr '
          let
            profiles = (builtins.fromTOML (builtins.readFile ./secretspec.toml)).profiles;
            keys = profile: let
              composed = map (spec: spec.composed or "") (builtins.attrValues profile);
              isInput = key: builtins.any (c: builtins.replaceStrings [("$" + "{" + key + "}")] [""] c != c) composed;
            in
              builtins.filter (key: !isInput key) (builtins.attrNames profile);
          in
            builtins.mapAttrs (_: keys) profiles
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
    host-switch = {
      exec = ''
        source ${common}

        # unlock once, secrets-build and the sudo password below reuse the session
        bw_unlock

        host=$(hostname)
        build_secrets "$host"

        # the login password is in bitwarden too, use it to unlock sudo; if it
        # doesn't match (yet), sudo below just prompts as usual
        if password=$(user_password "$host" "$USER"); then
          printf '%s\n' "$password" | sudo -S -p "" -v 2> /dev/null || true
        fi
        unset password

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

        # [node] refuse to deploy to a host whose ssh key differs from the repo (and so bitwarden)
        check_host_key() {
          local addr actual
          addr=$(nix eval --raw ".#deploy.nodes.$1.hostname")
          actual=$(ssh-keyscan -t ed25519 "$addr" 2> /dev/null | grep -v '^#' | cut -d' ' -f2-3 || true)
          if [ -z "$actual" ]; then
            echo "$1: could not read the host key of $addr" >&2
            exit 1
          fi
          if [ "$actual" != "$(pubkey "host/$1/ssh_host_ed25519_key.pub")" ]; then
            echo "$1: host key on $addr does not match host/$1/ssh_host_ed25519_key.pub" >&2
            exit 1
          fi
        }

        # [node] the ssh user's password is in bitwarden too, sudoExpect answers
        # deploy-rs's sudo prompts with it; nodes without one are asked on the terminal
        export_sudo_password() {
          local user password
          user=$(nix eval --raw ".#deploy.nodes.$1.profiles.system.sshUser")
          if password=$(user_password "$1" "$user"); then
            export "SUDO_PASSWORD_$1=$password"
          fi
        }

        # unlock once, secrets-build and the sudo passwords below reuse the session
        bw_unlock
        build_secrets "$@"

        targets=()
        for n in "$@"; do
          check_host_key "$n"
          export_sudo_password "$n"
          targets+=(".#$n")
        done

        # deploy-rs's flake check evaluates every host, but $secrets only holds the requested ones
        expect ${sudoExpect} deploy --skip-checks --targets "''${targets[@]}" -- --override-input secrets "path:$secrets"
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

        # only for zfs encrypted filesystems
        # install -d -m755 "$extra/persist/data/etc/zfs"
        # install -m600 "host/$host/disk.key" "$extra/persist/data/etc/zfs/disk.key"

        # assigned first, set -e ignores a failing $(...) inside the arguments below
        disko=$(build diskoScript)
        toplevel=$(build toplevel)

        nixos-anywhere \
          --disk-encryption-keys /tmp/disk.key "host/$host/disk.key" \
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
