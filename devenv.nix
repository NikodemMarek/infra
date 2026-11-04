{
  pkgs,
  lib,
  config,
  inputs,
  ...
}: let
  # Runs a command and answers deploy-rs's interactive sudo prompts with
  # the passwords read from fd 3 (lines of `host password`), asking on the terminal for hosts
  # without one. Read to the end before spawning, so the child inherits a drained pipe.
  sudoExpect = pkgs.writeText "deploy-sudo.exp" ''
    set timeout -1
    set passwords [open /dev/fd/3 r]
    foreach line [split [read $passwords] "\n"] {
      set space [string first " " $line]
      if {$space > 0} {
        set password([string range $line 0 [expr {$space - 1}]]) [string range $line [expr {$space + 1}] end]
      }
    }
    close $passwords

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

    # Bitwarden is only reached through the secret broker: `secret export` prints
    # {"<profile>": {"<KEY>": "<value>"}} after the user approved on the broker console.
    # $exported holds that json; it is never exported, so no child process sees it.

    # [host] {"<secret>": "<profile>"} of the secrets the host's config declares
    secret_profiles() { nix eval --json ".#nixosConfigurations.$1.config.secrets" --apply 'builtins.mapAttrs (_: s: s.profile)'; }

    # [host...] sets $exported to the secrets of all profiles the hosts' configs use
    export_hosts() {
      local h p profiles=""
      for h in "$@"; do
        # assigned first, set -e ignores a failing $(...) inside the arguments below
        p=$(secret_profiles "$h" | jq -r '[.[]] | unique[]')
        profiles+=$p$'\n'
      done
      # unquoted on purpose: one argument per profile, names are [a-z0-9-]
      profiles=$(printf '%s' "$profiles" | sort -u)
      exported=$(secret export $profiles)
    }

    # [profile] the exported secrets of one profile as json
    exported_profile() {
      printf '%s' "$exported" | jq --arg p "$1" '.[$p] // error("profile \($p) was not exported")'
    }

    # [host...] builds encrypted secrets for the hosts from $exported into $secrets
    build_secrets() {
      secrets=$(mktemp -d "''${XDG_RUNTIME_DIR:-/tmp}/secrets.XXXXXX")
      on_exit "rm -rf '$secrets'"
      printf '%s' "$exported" | secrets-build "$secrets" "$@"
    }

    # [host user] login password from $exported, fails if there is none
    user_password() {
      exported_profile "$1" 2> /dev/null | jq -er --arg k "USERS_''${2^^}_PASSWORD" '.[$k] // empty | select(. != "")'
    }

    # [file] key type and body of a public key (from stdin without a file), without the comment
    pubkey() { cut -d' ' -f1-2 "$@"; }

    # [anchor] the age recipient behind &anchor in .sops.yaml
    sops_recipient() { sed -n "s/.*&$1 \(age1[0-9a-z]*\).*/\1/p" .sops.yaml; }
  '';

  # Input: one profile of `secret export`. $names: the secrets taken from it.
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
    pkgs.wireguard-tools
    pkgs.qrencode
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
        secret_value() { printf '%s' "$1" | jq -r --arg k "$2" '.[$k]'; }

        # [host] the host's secrets from $exported as json, exactly the ones its config declares,
        # each taken from the profile that holds it
        fetch_secrets() {
          local profile_of profiles p names part parts=""
          # assigned first, set -e ignores a failing $(...) inside the arguments below
          profile_of=$(secret_profiles "$1")
          profiles=$(printf '%s' "$profile_of" | jq -r '[.[]] | unique[]')
          for p in $profiles; do
            names=$(printf '%s' "$profile_of" | jq -c --arg p "$p" '[to_entries[] | select(.value == $p) | .key]')
            part=$(exported_profile "$p" | jq --argjson names "$names" -f ${selectSecrets})
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
            if [ "$(secret_value "$2" "$key" | ssh-keygen -y -f /dev/stdin | pubkey)" != "$(pubkey "$file" 2> /dev/null || true)" ]; then
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
          have=$(secret_value "$2" flux_sops_age_key | age-keygen -y)
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
            hash=$(secret_value "$json" "$key" | mkpasswd -s)
            json=$(printf '%s' "$json" | jq --arg k "$key" --rawfile hash /dev/fd/3 '.[$k] = $hash' 3< <(printf '%s' "$hash"))
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

        # the callers pipe the exported json in, run by hand it is exported here
        if [ -t 0 ]; then
          export_hosts "$@"
        else
          exported=$(cat)
        fi
        if [ -z "$exported" ]; then
          echo "no exported secrets on stdin" >&2
          exit 1
        fi

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
      description = "[outdir host...] build encrypted sops files from bitwarden (exported json on stdin)";
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

        # one console approval for the cluster key and every namespace
        profiles=(dijkstra)
        for ns in $namespaces; do
          profiles+=("dijkstra-$ns")
        done
        exported=$(secret export "''${profiles[@]}")

        # the cluster key decrypts the existing files, it must be the one behind &dijkstra_cluster
        key=$(exported_profile dijkstra | jq -r '.FLUX_SOPS_AGE_KEY // empty')
        if [ -z "$key" ]; then
          echo "FLUX_SOPS_AGE_KEY is missing or empty in bitwarden" >&2
          exit 1
        fi
        have=$(printf '%s\n' "$key" | age-keygen -y)
        want=$(sops_recipient dijkstra_cluster)
        if [ "$have" != "$want" ]; then
          echo "FLUX_SOPS_AGE_KEY is $have, but .sops.yaml has ''${want:-nothing}" >&2
          exit 1
        fi

        # plaintext only ever lives in shell variables and pipes (printf is a builtin, so
        # never argv), the key reaches sops through a pipe on fd 3; $(...) strips trailing
        # newlines, harmless for a whole json document as the values are escaped inside it
        for ns in $namespaces; do
          file=$dir/$ns.enc.yaml
          if ! listed "$ns" "$nsmanifests"; then
            echo "$ns: no kind: Namespace manifest under clusters/dijkstra" >&2
            exit 1
          fi

          names=$(printf '%s' "$declared" | jq -c --arg ns "$ns" '.[$ns]')
          secret=$(exported_profile "dijkstra-$ns" | jq --argjson names "$names" -f ${selectSecrets} | jq --arg ns "$ns" -f ${renderSecret})
          wanted=$(printf '%s' "$secret" | jq -S .)

          status=new
          if [ -f "$file" ]; then
            # the recipients are plaintext metadata; a file for another key (rotation) is rewritten
            recipients=$(yq '.sops.age[].recipient' "$file")
            # a failing decrypt must abort, not count as a change
            if ! current=$(SOPS_AGE_KEY_FILE=/dev/fd/3 sops decrypt --output-type json "$file" 3< <(printf '%s\n' "$key") | jq -S '{apiVersion, kind, metadata: {name: .metadata.name, namespace: .metadata.namespace}, type, stringData}'); then
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

        host=$(hostname)
        # one console approval, secrets-build and the sudo password below reuse the result
        export_hosts "$host"
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

        # [node] the ssh user's password is in bitwarden too, it is added to $passwords as a
        # `node password` line for sudoExpect to answer deploy-rs's sudo prompts with;
        # nodes without one are asked on the terminal
        add_sudo_password() {
          local user password
          user=$(nix eval --raw ".#deploy.nodes.$1.profiles.system.sshUser")
          if password=$(user_password "$1" "$user"); then
            passwords+="$1 $password"$'\n'
          fi
        }

        # one console approval, secrets-build and the sudo passwords below reuse the result
        export_hosts "$@"
        build_secrets "$@"

        passwords=""
        targets=()
        for n in "$@"; do
          check_host_key "$n"
          add_sudo_password "$n"
          targets+=(".#$n")
        done

        # deploy-rs's flake check evaluates every host, but $secrets only holds the requested ones
        expect ${sudoExpect} deploy --skip-checks --targets "''${targets[@]}" -- --override-input secrets "path:$secrets" 3< <(printf '%s' "$passwords")
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

        export_hosts "$host"
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
