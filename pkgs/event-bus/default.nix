{
  writeShellApplication,
  symlinkJoin,
  natscli,
  jq,
  coreutils,
}:

# Events over plain NATS (local nats-server, no JetStream, $NATS_URL or 127.0.0.1:4222).
# `event-pub TYPE` reads a JSON object from stdin (empty means {}), sets its `type` to
# TYPE and publishes it on `events.TYPE`.
# `event-subscribe HANDLER SUBJECT...` subscribes to the subjects and runs HANDLER for
# every message that is a JSON object with a valid `type`: the message on stdin,
# the type in $ER_TYPE. The handlers run in the background; natscli reconnects by
# itself, so event-subscribe only exits when `nats sub` does (the service restarts it).
let
  eventPub = writeShellApplication {
    name = "event-pub";
    runtimeInputs = [
      natscli
      jq
      coreutils
    ];
    text = ''
      type_re='^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'

      if [[ $# -ne 1 ]] || ! [[ $1 =~ $type_re ]]; then
        echo "usage: event-pub TYPE (TYPE matches $type_re; a JSON object on stdin)" >&2
        exit 2
      fi
      type=$1

      # slurped: whitespace-only stdin gives [], several JSON values give a longer array
      payload=$(jq -cs --arg t "$type" '
        if length == 0 then {}
        elif length == 1 and (.[0] | type) == "object" then .[0]
        else empty end
        | .type = $t
      ' 2>/dev/null) || payload=""
      if [[ -z $payload ]]; then
        echo "event-pub: stdin must be a single JSON object" >&2
        exit 2
      fi

      if ! err=$(printf '%s' "$payload" | timeout 4 nats --server "''${NATS_URL:-nats://127.0.0.1:4222}" pub --no-templates "events.$type" 2>&1 >/dev/null); then
        echo "event-pub: cannot publish events.$type: $err" >&2
        exit 1
      fi
    '';
  };

  eventSubscribe = writeShellApplication {
    name = "event-subscribe";
    runtimeInputs = [
      natscli
      jq
      coreutils
    ];
    text = ''
      type_re='^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$'

      if [[ $# -lt 2 ]]; then
        echo "usage: event-subscribe HANDLER SUBJECT..." >&2
        exit 2
      fi
      handler=$1
      shift

      cd /

      # the trap comes first, so a TERM during startup cannot orphan nats sub
      trap 'kill "''${sub:-}" 2>/dev/null; exit 0' TERM INT

      # exec: $! is the pid of nats itself, so the trap kills the right process
      exec 3< <(exec nats --server "''${NATS_URL:-nats://127.0.0.1:4222}" sub --raw "$@")
      sub=$!

      while IFS= read -r line <&3; do
        # slurped, so only a line holding exactly one JSON object passes; natscli
        # prints `nil body` for empty bodies; non-JSON, non-objects, several values
        # and pieces of multi-line payloads fail here too
        t=$(jq -rs 'if length == 1 and (.[0] | type) == "object" then .[0].type | strings else empty end' <<<"$line" 2>/dev/null) || continue
        [[ $t =~ $type_re ]] || continue

        # a subshell, so that the loop never waits for the handler; fd 3 is not passed on
        (
          rc=0
          ER_TYPE=$t "$handler" <<<"$line" || rc=$?
          if ((rc != 0)); then
            echo "event-subscribe: ''${handler##*/} exited with $rc ($t)" >&2
          fi
        ) 3<&- &
      done

      echo "event-subscribe: nats sub exited" >&2
      exit 1
    '';
  };

in
symlinkJoin {
  name = "event-bus";
  paths = [
    eventPub
    eventSubscribe
  ];
}
