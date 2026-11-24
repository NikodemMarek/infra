{
  lib,
  writeShellApplication,
  jq,
  opentofu,
  bws-project,
}: let
  keys = builtins.attrNames (builtins.fromTOML (builtins.readFile ../../secretspec.toml)).profiles.tofu;
in
  # OpenTofu in the current directory with the secrets of the Bitwarden Secrets
  # Manager project `tofu` (the keys of the secretspec profile `tofu`) in its environment.
  writeShellApplication {
    name = "infra-tofu";
    runtimeInputs = [bws-project jq opentofu];
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
        keys=(${lib.escapeShellArgs keys})

        secrets=$(bws-project tofu)

        # names match the keys case-insensitively
        printf '%s' "$secrets" | jq --args '
          . as $secrets
          | [$ARGS.positional[] | select(($secrets[ascii_downcase] // "") == "")] as $missing
          | if $missing != [] then error("missing or empty keys in bws project tofu: \($missing | join(", "))") else empty end
        ' "''${keys[@]}"

        # plaintext only ever lives in memory and pipes, never touches the disk
        while IFS= read -r -d "" pair; do
          declare -x "$pair"
        done < <(printf '%s' "$secrets" | jq -j --args '
          . as $secrets
          | $ARGS.positional[]
          | "\(.)=\($secrets[ascii_downcase])\u0000"
        ' "''${keys[@]}")
        unset secrets BWS_ACCESS_TOKEN
        # OCI's S3-compatible API rejects the AWS SDK's default checksums
        export AWS_REQUEST_CHECKSUM_CALCULATION=when_required AWS_RESPONSE_CHECKSUM_VALIDATION=when_required
      fi

      exec tofu "$@"
    '';
  }
