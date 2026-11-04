{
  writeShellApplication,
  lib,
  coreutils,
  findutils,
  jq,
}:

# event handler: queues the event for agent-inbox as
# $XDG_RUNTIME_DIR/agent-inbox/<id>.json = {"id", "received", "event"}. An event with
# a non-empty string `key` has the id `<h>-<r>`: h is a hash of its type and key, r is
# random, so every event gets a fresh id (a dismissal based on a stale listing can't
# delete a newer item) and replaces the queued items of the same type and key that are
# not newer (a status re-sent every turn); otherwise the id is 16 random hex digits. At
# most 200 items are kept, the oldest go first. Expiry (`timeout`, `reply`) is up to
# the reader.
let
  inbox = writeShellApplication {
    name = "event-handler-inbox";
    runtimeInputs = [
      jq
      coreutils
      findutils
    ];
    text = ''
      event=$(cat)

      if ! jq -es 'length == 1 and (.[0] | type == "object") and (.[0].type | type == "string")' <<<"$event" >/dev/null 2>&1; then
        echo "inbox: the event must be a JSON object with a string 'type'" >&2
        exit 1
      fi
      if [[ -z "''${XDG_RUNTIME_DIR:-}" ]]; then
        echo "inbox: XDG_RUNTIME_DIR is not set" >&2
        exit 1
      fi

      type=$(jq -r '.type' <<<"$event")
      key=$(jq -r '.key | strings' <<<"$event")
      if [[ -n $key ]]; then
        h=$(printf '%s\n%s' "$type" "$key" | sha256sum | cut -c1-16)
        id=$h-$(head -c4 /dev/urandom | od -An -tx1 | tr -d ' \n')
      else
        id=$(head -c8 /dev/urandom | od -An -tx1 | tr -d ' \n')
      fi

      dir=$XDG_RUNTIME_DIR/agent-inbox
      mkdir -p "$dir"
      chmod 700 "$dir"

      # a hidden temp file, then mv: the reader never sees a partial item
      tmp=$(mktemp "$dir/.tmp.XXXXXX")
      trap 'rm -f -- "$tmp"' EXIT
      jq -c --arg id "$id" --argjson received "$(date +%s.%N)" '{id: $id, received: $received, event: .}' <<<"$event" >"$tmp"
      mv -T -- "$tmp" "$dir/$id.json"

      # || true: a concurrent handler or pruner must not fail this one
      if [[ -n $key ]]; then
        find "$dir" -ignore_readdir_race -maxdepth 1 -name "$h-*.json" ! -name "$id.json" ! -newer "$dir/$id.json" -delete 2>/dev/null || true
      fi

      # keep the newest 200
      {
        find "$dir" -ignore_readdir_race -maxdepth 1 -type f -name '*.json' -printf '%T@ %p\n' \
          | sort -rn | tail -n +201 | cut -d' ' -f2- | xargs -r -d '\n' rm -f --
      } || true
    '';
  };
in
inbox
