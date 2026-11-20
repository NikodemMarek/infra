{
  lib,
  stdenvNoCC,
  fetchzip,
  writeShellApplication,
  jq,
  opentofu,
}: let
  # the release binary: nixpkgs' bws is unfree, so it is not cached and builds from source
  bws = let
    version = "2.1.0";
    arch = stdenvNoCC.hostPlatform.parsed.cpu.name;
    hashes = {
      x86_64 = "sha256-p79b+8ugF9yqCq9XRC9mOHI5wPUWrJ6lFa20pi8bW4M=";
      aarch64 = "sha256-/OpgX+kY/71NXcFyATX3n4w9mjxk3P92fhkIJZPj+P0=";
    };
  in
    stdenvNoCC.mkDerivation {
      pname = "bws";
      inherit version;
      src = fetchzip {
        url = "https://github.com/bitwarden/sdk-sm/releases/download/bws-v${version}/bws-${arch}-unknown-linux-musl-${version}.zip";
        hash = hashes.${arch};
        stripRoot = false;
      };
      installPhase = "install -Dm755 bws $out/bin/bws";
    };
  tofu = opentofu.withPlugins (p: [p.cloudflare_cloudflare p.tailscale_tailscale p.oracle_oci]);
  keys = builtins.attrNames (builtins.fromTOML (builtins.readFile ../../secretspec.toml)).profiles.tofu;
in
  # OpenTofu in the current directory with the secrets of the Bitwarden Secrets
  # Manager project `tofu` (the keys of the secretspec profile `tofu`) in its environment.
  writeShellApplication {
    name = "infra-tofu";
    runtimeInputs = [bws jq tofu];
    text = ''
      direct=false
      case "''${1:-}" in
        fmt | version | validate) direct=true ;;
        init)
          case " $* " in
            *" -backend=false "*) direct=true ;;
          esac
          ;;
      esac

      if ! $direct; then
        if [ -z "''${BWS_ACCESS_TOKEN:-}" ]; then
          echo "BWS_ACCESS_TOKEN is not set" >&2
          exit 1
        fi

        keys=(${lib.escapeShellArgs keys})

        projects=$(bws project list --output json)
        project=$(printf '%s' "$projects" | jq -r '
          [.[] | select(.name == "tofu") | .id]
          | if length == 1 then .[0] else error("expected one bws project named tofu, found \(length)") end
        ')
        # names match the keys case-insensitively
        secrets=$(bws secret list "$project" --output json | jq --args '
          ($ARGS.positional | map({key: ascii_downcase, value: .}) | from_entries) as $names
          | map(.key |= ($names[ascii_downcase] // ascii_downcase))
        ' "''${keys[@]}")
        unset projects project

        printf '%s' "$secrets" | jq --args '
          $ARGS.positional as $keys
          | (group_by(.key) | map(select(length > 1) | .[0].key)) as $duplicate
          | ([.[] | select(.key | IN($keys[])) | select(.value != null and .value != "") | .key]) as $present
          | ($keys - $present) as $missing
          | if $duplicate != [] then error("duplicate keys in bws project tofu: \($duplicate | join(", "))")
            elif $missing != [] then error("missing or empty keys in bws project tofu: \($missing | join(", "))")
            else empty end
        ' "''${keys[@]}"

        if [ "''${GITHUB_ACTIONS:-}" = true ]; then
          printf '%s' "$secrets" | jq -r --args '
            $ARGS.positional as $keys
            | .[] | select(.key | IN($keys[])) | .value | split("\n")[] | select(. != "")
            | "::add-mask::\(.)"
          ' "''${keys[@]}" >&2
        fi

        # plaintext only ever lives in memory and pipes, never touches the disk
        while IFS= read -r -d "" pair; do
          declare -x "$pair"
        done < <(printf '%s' "$secrets" | jq -j --args '
          $ARGS.positional as $keys
          | .[] | select(.key | IN($keys[])) | "\(.key)=\(.value)\u0000"
        ' "''${keys[@]}")
        unset secrets BWS_ACCESS_TOKEN
        # OCI's S3-compatible API rejects the AWS SDK's default checksums
        export AWS_REQUEST_CHECKSUM_CALCULATION=when_required AWS_RESPONSE_CHECKSUM_VALIDATION=when_required
      fi

      exec tofu "$@"
    '';
  }
