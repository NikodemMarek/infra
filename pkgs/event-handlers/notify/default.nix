{
  writeShellApplication,
  libnotify,
  xdg-utils,
  jq,
  # low | normal | critical
  urgency ? "normal",
}:

# event handler: desktop notification, "Open" action opens the event URL.
writeShellApplication {
  name = "event-handler-notify-${urgency}";
  runtimeInputs = [
    libnotify
    xdg-utils
    jq
  ];
  text = ''
    event=$(cat)

    summary=$(jq -r '"\(.project // .host): \(.type | split(".") | last | gsub("_"; " "))"' <<<"$event")
    # notification bodies are markup, hence @html
    body=$(jq -r '
      [ .title, (if .author then "by @\(.author)" else empty end),
        (if (.shpool | type) == "string" and .shpool != "" then "shpool attach \(.shpool)" else empty end),
        ((.body // "") | .[0:300]) ]
      | map(select(. != null and . != "") | @html) | join("\n")
    ' <<<"$event")
    url=$(jq -r '.url // ""' <<<"$event")

    # detach: notify-send -A blocks until the notification is closed
    (
      action=$(notify-send -a event-router -u ${urgency} -A open=Open "$summary" "$body" || true)
      if [[ "$action" == open && -n "$url" ]]; then
        xdg-open "$url"
      fi
    ) </dev/null >/dev/null 2>&1 &
  '';
}
