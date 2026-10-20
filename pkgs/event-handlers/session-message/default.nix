{
  writeShellApplication,
  coreutils,
  findutils,
  jq,
}:

# event handler: `session.message {to, from?, text}` goes into the mailbox of the
# session `to`: $XDG_RUNTIME_DIR/claude-mail/<to>/queue/<unix-nanos>-<pid>.md,
# which the Claude Code hooks of that session deliver (see claude-mail-take).
# The text is quoted line by line, so it cannot pass for a `Message from ...:` header.
writeShellApplication {
  name = "event-handler-session-message";
  runtimeInputs = [
    jq
    coreutils
    findutils
  ];
  text = ''
    event=$(cat)

    # \z, not $: the regex of jq also matches before a trailing newline
    if ! jq -e '(.to | type == "string") and (.to | test("^[A-Za-z0-9][A-Za-z0-9._-]*\\z"))' <<<"$event" >/dev/null 2>&1; then
      echo "session.message: 'to' must match [A-Za-z0-9][A-Za-z0-9._-]*" >&2
      exit 1
    fi
    if ! jq -e '(.text | type == "string") and .text != ""' <<<"$event" >/dev/null 2>&1; then
      echo "session.message: 'text' must be a non-empty string" >&2
      exit 1
    fi
    if [[ -z "''${XDG_RUNTIME_DIR:-}" ]]; then
      echo "session.message: XDG_RUNTIME_DIR is not set" >&2
      exit 1
    fi

    to=$(jq -r '.to' <<<"$event")
    from=$(jq -r '((.from | strings) // "") | gsub("[^A-Za-z0-9._-]"; "") | .[0:64] | if . == "" then "unknown" else . end' <<<"$event")

    mail=$XDG_RUNTIME_DIR/claude-mail
    box=$mail/$to
    mkdir -p "$mail"
    chmod 700 "$mail"
    mkdir -p "$box/queue"

    # a temp file in the mailbox dir (outside queue/), then mv: the queue never holds a partial file
    tmp=$(mktemp "$box/.tmp.XXXXXX")
    trap 'rm -f -- "$tmp"' EXIT
    # every line break (also \r, U+2028/2029, U+0085) becomes \n and other control characters are dropped, so each visual line is quoted
    jq -j --arg from "$from" '
      "Message from \($from):\n\n"
        + (.text | gsub("\r\n|\r| | |\u0085"; "\n") | gsub("(?![\\t\\n])\\p{Cc}"; "") | split("\n") | map("> " + .) | join("\n"))
        + "\n"
      | if length > 8000 then .[0:8000] + "\n[truncated]" else . end
    ' <<<"$event" >"$tmp"
    mv -T -- "$tmp" "$box/queue/$(date +%s%N)-$$.md"

    # -ignore_readdir_race and || true: a concurrent pruner must not fail the handler
    find "$mail" -ignore_readdir_race -mindepth 1 -maxdepth 1 -type d -mmin +2880 -exec rm -rf -- {} + || true
  '';
}
