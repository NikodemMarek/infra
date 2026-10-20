{
  writeShellApplication,
  coreutils,
  libnotify,
  xdg-utils,
  jq,
  # low | normal | critical
  urgency ? "normal",
}:

# event handler: desktop notification, "Open" action opens the event URL
# (or the url of the session's origin file).
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

    summary=$(jq -r '"\(.project // .host): \(.type | split(".") | last | gsub("_"; " "))"' <<<"$event")
    # notification bodies are markup, hence @html
    body=$(jq -r '
      [ .title, (if .author then "by @\(.author)" else empty end),
        (if (.shpool | type) == "string" and .shpool != "" then "shpool attach \(.shpool)" else empty end),
        ((.body // "") | .[0:300]) ]
      | map(select(. != null and . != "") | @html) | join("\n")
    ' <<<"$event")
    url=$(jq -r '.url // ""' <<<"$event")
    if [[ -z "$url" ]]; then
      # the path comes from an event: only read files directly in the origin dir
      origin_file=$(jq -r '.origin_file // ""' <<<"$event" || true)
      origin_dir=''${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/claude-origin
      if [[ -n "$origin_file" && "''${origin_file%/*}" == "$origin_dir" && -f "$origin_file" ]]; then
        url=$(jq -r '.url // empty' "$origin_file" 2>/dev/null || true)
      fi
    fi

    # detach: notify-send -A blocks until the notification is closed
    (
      action=$(notify-send -a event-router -u ${urgency} -A open=Open "$summary" "$body" || true)
      if [[ "$action" == open && -n "$url" ]]; then
        xdg-open "$url"
      fi
    ) </dev/null >/dev/null 2>&1 &
  '';
}
