{
  writeShellApplication,
  age,
  jq,
  mkpasswd,
  openssh,
  sops,
  ssh-to-age,
  bws-project,
}:
# Encrypted sops files for hosts from the Bitwarden Secrets Manager project `hosts`,
# run from the repo root, needs BWS_ACCESS_TOKEN.
writeShellApplication {
  name = "secrets-build";
  runtimeInputs = [age bws-project jq mkpasswd openssh sops ssh-to-age];
  text = ''
    # also fail inside $(...), so a failing step in a function aborts the script
    shopt -s inherit_errexit

    if [ "$#" -lt 2 ]; then
      echo "usage: secrets-build outdir host..."
      exit 1
    fi

    # [json key] value of one secret
    secret() { printf '%s' "$1" | jq -r --arg k "$2" '.[$k]'; }

    # [file] key type and body of a public key (from stdin without a file), without the comment
    pubkey() { cut -d' ' -f1-2 "$@"; }

    # [anchor] the age recipient behind &anchor in .sops.yaml
    sops_recipient() { sed -n "s/.*&$1 \(age1[0-9a-z]*\).*/\1/p" .sops.yaml; }

    # [host] the host's secrets from bws as json, exactly the ones its config declares,
    # each taken from the profile that holds it
    fetch_secrets() {
      local profile_of profiles p names part parts=""
      # assigned first, set -e ignores a failing $(...) inside the arguments below
      profile_of=$(nix eval --json ".#nixosConfigurations.$1.config.secrets" --apply 'builtins.mapAttrs (_: s: s.profile)')
      profiles=$(printf '%s' "$profile_of" | jq -r '[.[]] | unique[]')
      for p in $profiles; do
        names=$(printf '%s' "$profile_of" | jq -c --arg p "$p" '[to_entries[] | select(.value == $p) | .key]')
        part=$(printf '%s' "$hosts" | jq --arg p "$p" --argjson names "$names" --argjson composed "$composed" -f ${./profile-secrets.jq} | jq --argjson names "$names" -f ${./select-secrets.jq}) || {
          echo "$1: profile $p" >&2
          exit 1
        }
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
        echo "$1: flux_sops_age_key in bws is $have, but .sops.yaml has ''${want:-nothing}" >&2
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
    hosts=$(bws-project hosts)
    # {"<profile>": {"<KEY>": "<template>"}} of the composed secrets
    composed=$(nix eval --impure --json --expr '(builtins.fromTOML (builtins.readFile ./secretspec.toml)).profiles' \
      | jq 'map_values(with_entries(select(.value.composed) | .value |= .composed))')

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
}
