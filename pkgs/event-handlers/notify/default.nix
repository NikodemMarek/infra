{
  writeShellApplication,
  coreutils,
  libnotify,
  xdg-utils,
  jq,
  jw,
  # low | normal | critical
  urgency ? "normal",
}:
# event handler: desktop notification, "Open" action opens the event URL
# (or the url of the session's record, else its first MR), if it is an https URL.
writeShellApplication {
  name = "event-handler-notify-${urgency}";
  runtimeInputs = [
    libnotify
    xdg-utils
    jq
    coreutils
  ];
  text = ''
    event=$(cat)
    export PATH="$PATH:${jw}/bin"

    summary=$(jq -r '"\(.project // .host): \(.type | split(".") | last | gsub("_"; " "))"' <<<"$event")
    # notification bodies are markup, hence @html
    body=$(jq -r '
      [ .title, (if .author then "by @\(.author)" else empty end),
        ((.body // "") | .[0:300]) ]
      | map(select(. != null and . != "") | @html) | join("\n")
    ' <<<"$event")
    url=$(jq -r '.url | strings // ""' <<<"$event")
    if [[ -z "$url" ]]; then
      # the name comes from an event: build a path only from a valid one
      session=$(jq -r '.session | strings | select(test("^[A-Za-z0-9][A-Za-z0-9._-]*\\z"))' <<<"$event" || true)
      if [[ -n "$session" ]]; then
        url=$(jw session get -s "$session" 2>/dev/null |
          jq -r '(.url | strings | select(. != "")) // (.mrs | arrays | first | strings) // ""' 2>/dev/null || true)
      fi
    fi

    # only a plain https URL is opened (\z, not $: the regex of jq also matches before a trailing newline);
    # the whole string is tested, so a URL with a newline is rejected
    url=$(jq -rn --arg u "$url" '$u | select(length <= 2048 and test("^https://[!-~]+\\z"))' || true)

    # detach: notify-send -A blocks until the notification is closed
    (
      action=$(notify-send -a events -u ${urgency} -A open=Open -- "$summary" "$body" || true)
      if [[ "$action" == open && -n "$url" ]]; then
        xdg-open "$url"
      fi
    ) </dev/null >/dev/null 2>&1 &
  '';
}
