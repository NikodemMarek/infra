{
  writeShellApplication,
  bws,
  jq,
}:
# The secrets of the Bitwarden Secrets Manager project `name` as one JSON object
# {"<key, lowercased>": "<value>"}, needs BWS_ACCESS_TOKEN.
writeShellApplication {
  name = "bws-project";
  runtimeInputs = [bws jq];
  text = ''
    if [ "$#" -ne 1 ]; then
      echo "usage: bws-project name" >&2
      exit 1
    fi
    name=$1

    if [ -z "''${BWS_ACCESS_TOKEN:-}" ]; then
      echo "BWS_ACCESS_TOKEN is not set" >&2
      exit 1
    fi

    project=$(bws project list --output json | jq -r --arg name "$name" '
      [.[] | select(.name == $name) | .id]
      | if length == 1 then .[0] else error("expected one bws project named \($name), found \(length)") end
    ')

    secrets=$(bws secret list "$project" --output json | jq --arg name "$name" '
      map({key: (.key | ascii_downcase), value})
      | (group_by(.key) | map(select(length > 1) | .[0].key)) as $duplicate
      | if $duplicate != [] then error("duplicate keys in bws project \($name): \($duplicate | join(", "))") else from_entries end
    ')

    if [ "''${GITHUB_ACTIONS:-}" = true ]; then
      printf '%s' "$secrets" | jq -r '
        .[] | strings | splits("[\r\n]") | select(. != "")
        | "::add-mask::\(gsub("%"; "%25"))"
      ' >&2
    fi

    printf '%s' "$secrets"
  '';
}
