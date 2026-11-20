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

    # [host...] builds encrypted secrets for the hosts into $secrets
    build_secrets() {
      secrets=$(mktemp -d "''${XDG_RUNTIME_DIR:-/tmp}/secrets.XXXXXX")
      on_exit "rm -rf '$secrets'"
      secrets-build "$secrets" "$@"
    }

    # [host user] login password from bws, fails if there is none
    user_password() {
      nix run "$DEVENV_ROOT#bws-project" -- hosts 2> /dev/null \
        | jq -er --arg k "''${1,,}/users_''${2,,}_password" '.[$k] // empty'
    }

    # [file] key type and body of a public key (from stdin without a file), without the comment
    pubkey() { cut -d' ' -f1-2 "$@"; }

    # [anchor] the age recipient behind &anchor in .sops.yaml
    sops_recipient() { sed -n "s/.*&$1 \(age1[0-9a-z]*\).*/\1/p" .sops.yaml; }
  '';

  # Input: the secrets of the bws project `hosts` (lowercase `<profile>/<key>` names).
  # $p: the profile. $names: the secrets taken from it. $composed: the `composed` templates of every
  # profile (secretspec.toml), by profile and key.
  # Output: the secrets of the profile under their plain names, plus the composed ones in $names with
  # their ${KEY} inputs substituted; fails if an input is missing or empty.
  profileSecrets = pkgs.writeText "profile-secrets.jq" ''
    with_entries(select(.key | startswith($p + "/")) | .key |= ltrimstr($p + "/")) as $vals
    | (
      ($composed[$p] // {})
      | to_entries
      | map(select(.key | ascii_downcase | IN($names[])))
      | map(
        .value as $template
        | ([$template | match("\\$\\{(?<k>[A-Za-z0-9_]+)\\}"; "g").captures[0].string | ascii_downcase | select(($vals[.] // "") == "")] | unique) as $bad
        | if $bad != [] then error("\($p)/\(.key) needs missing or empty \($bad)")
          else {key: (.key | ascii_downcase), value: ($template | gsub("\\$\\{(?<k>[A-Za-z0-9_]+)\\}"; $vals[.k | ascii_downcase]))}
          end
      )
      | from_entries
    ) as $rendered
    | $vals + $rendered
  '';

  # Input: the secrets of one profile. $names: the secrets taken from it.
  # Output: exactly those secrets, with lowercase names; fails if one is missing or empty.
  selectSecrets = pkgs.writeText "select-secrets.jq" ''
    with_entries(.key |= ascii_downcase)
    | ($names - keys) as $missing
    | if $missing != [] then error("missing in bws: \($missing)") end
    | with_entries(select(.key | IN($names[])))
    | [to_entries[] | select(.value == null or .value == "") | .key] as $empty
    | if $empty != [] then error("empty in bws: \($empty)") end
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
    pkgs.mkpasswd
    pkgs.expect

    pkgs.kubectl
    pkgs.fluxcd
    pkgs.velero
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
        source ${common}

        if [ "$#" -lt 2 ]; then
          echo "usage: secrets-build outdir host..."
          exit 1
        fi

        # [json key] value of one secret
        secret() { printf '%s' "$1" | jq -r --arg k "$2" '.[$k]'; }

        # [host profile names] the secrets `names` (json list) of the profile from bws as json
        profile_part() {
          printf '%s' "$hosts" | jq --arg p "$2" --argjson names "$3" --argjson composed "$composed" -f ${profileSecrets} | jq --argjson names "$3" -f ${selectSecrets} || {
            echo "$1: profile $2" >&2
            exit 1
          }
        }

        # [host] the host's secrets from bws as json, exactly the ones its config declares,
        # each taken from the profile that holds it; a whole profile secret is all the keys of
        # its profile as one json string
        fetch_secrets() {
          local used p name names part parts=""
          # assigned first, set -e ignores a failing $(...) inside the arguments below
          used=$(nix eval --json ".#nixosConfigurations.$1.config.secrets" --apply 'builtins.mapAttrs (_: s: {inherit (s) profile wholeProfile;})')
          for p in $(printf '%s' "$used" | jq -r '[.[] | select(.wholeProfile | not) | .profile] | unique[]'); do
            names=$(printf '%s' "$used" | jq -c --arg p "$p" '[to_entries[] | select((.value.wholeProfile | not) and .value.profile == $p) | .key]')
            part=$(profile_part "$1" "$p" "$names")
            parts+=$part$'\n'
          done
          for name in $(printf '%s' "$used" | jq -r 'to_entries[] | select(.value.wholeProfile) | .key'); do
            p=$(printf '%s' "$used" | jq -r --arg n "$name" '.[$n].profile')
            names=$(printf '%s' "$specs" | jq -c --arg p "$p" '.[$p] | keys | map(ascii_downcase)')
            part=$(profile_part "$1" "$p" "$names" | jq --arg n "$name" '{($n): tojson}')
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
              echo "$1: $key in bws does not match $file" >&2
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

        # fetched once for all hosts
        hosts=$(nix run "$DEVENV_ROOT#bws-project" -- hosts)
        specs=$(nix eval --impure --json --expr '(builtins.fromTOML (builtins.readFile ./secretspec.toml)).profiles')
        # {"<profile>": {"<KEY>": "<template>"}} of the composed secrets
        composed=$(printf '%s' "$specs" | jq 'map_values(with_entries(select(.value.composed) | .value |= .composed))')

        # plaintext only ever lives in memory and pipes, never touches the disk
        for h in "$@"; do
          json=$(fetch_secrets "$h")
          check_ssh_keys "$h" "$json"
          check_sops_identity "$h"
          json=$(hash_passwords "$json")
          encrypt "$h" "$json" > "$outdir/.$h.yaml.tmp"
          mv "$outdir/.$h.yaml.tmp" "$outdir/$h.yaml"
        done
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

        # the login password is in bws too, use it to unlock sudo; if it
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

        # [node] the ssh user's password is in bws too, sudoExpect answers
        # deploy-rs's sudo prompts with it; nodes without one are asked on the terminal
        export_sudo_password() {
          local user password
          user=$(nix eval --raw ".#deploy.nodes.$1.profiles.system.sshUser")
          if password=$(user_password "$1" "$user"); then
            export "SUDO_PASSWORD_$1=$password"
          fi
        }

        build_secrets "$@"

        known_hosts=$(mktemp)
        on_exit "rm -f '$known_hosts'"
        nix eval --raw .#knownHosts > "$known_hosts"

        targets=()
        for n in "$@"; do
          export_sudo_password "$n"
          targets+=(".#$n")
        done

        # deploy-rs's flake check evaluates every host, but $secrets only holds the requested ones
        expect ${sudoExpect} deploy --skip-checks \
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
