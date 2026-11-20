{
  writeShellApplication,
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
  runtimeInputs = [bws-project jq mkpasswd openssh sops ssh-to-age];
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

    # [host profile names] the secrets `names` (json list) of the profile from bws as json
    profile_part() {
      printf '%s' "$hosts" | jq --arg p "$2" --argjson names "$3" --argjson composed "$composed" -f ${./profile-secrets.jq} | jq --argjson names "$3" -f ${./select-secrets.jq} || {
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
        part=$(profile_part "$1" "$p" "$names" | jq --arg n "$name" '{($n): (to_entries | sort_by(.key) | from_entries | tojson)}')
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
    hosts=$(bws-project hosts)
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
}
