{
  lib,
  writeShellApplication,
  jq,
  notify,
}:

# event handler for `remote.notify.>` messages that come in over the leaf link
# from the cluster, so untrusted: only `project`, `title` and `body` are kept
# (in that shape, with a fixed type) and handed to notify. Everything else, above
# all `url` (notify would open it), is dropped.
let
  handler = writeShellApplication {
    name = "event-handler-remote-notify";
    runtimeInputs = [ jq ];
    text = ''
      event=$(cat)

      # Control characters and line/paragraph separators become spaces and
      # bidi/zero-width controls are dropped, before cutting: notify adds lines
      # of its own to the body, which a title must not be able to imitate.
      # \A and \z, not ^ and $: the regex of jq also matches before a trailing newline.
      if ! event=$(jq -ces '
        def clean:
          explode
          | map(select(((. >= 8203 and . <= 8207) or (. >= 8234 and . <= 8238) or (. >= 8294 and . <= 8297) or . == 65279) | not))
          | map(if . < 32 or . == 127 or . == 133 or . == 8232 or . == 8233 then 32 else . end)
          | implode;
        if length == 1 and (.[0] | type == "object") and (.[0].title | type == "string") then .[0] else empty end
        | {
            type: "remote.notify",
            project: (if (.project | type == "string") and (.project | test("\\A[A-Za-z0-9._-]{1,64}\\z")) then .project else "remote" end),
            title: (.title | clean | .[0:200])
          } + (if (.body | type) == "string" then {body: (.body | clean | .[0:1000])} else {} end)
        | select(.title | test("\\S"))
      ' <<<"$event" 2>/dev/null); then
        echo "remote.notify: the event must be one JSON object with a non-empty string 'title'" >&2
        exit 1
      fi

      ${lib.getExe notify} <<<"$event"
    '';
  };
in
handler
