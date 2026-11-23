{
  writeShellApplication,
  dictate,
  wtype,
  libnotify,
  coreutils,
}:
# Toggle for a keybind: the first run records (dictate), the second run stops the
# recording by signalling the first, which then types the text into the focused window.
writeShellApplication {
  name = "dictate-type";
  runtimeInputs = [
    dictate
    wtype
    libnotify
    coreutils
  ];
  text = ''
    pidfile=$XDG_RUNTIME_DIR/dictate-type.pid

    if [[ -f "$pidfile" ]] && kill -0 "$(<"$pidfile")" 2>/dev/null; then
      kill -TERM "$(<"$pidfile")"
      exit 0
    fi

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"; rm -f "$pidfile"' EXIT
    echo $$ >"$pidfile"

    id=$(notify-send -p -a dictate -t 0 "Dictating…" "Press SUPER+D to stop")

    dictate >"$tmp/out" 2>"$tmp/err" &
    rec=$!

    # Forward the stop to dictate; ignore further presses so they cannot cancel transcription.
    stop() {
      trap "" TERM
      kill -TERM "$rec" 2>/dev/null || true
    }
    trap stop TERM

    rc=0
    wait "$rec" || rc=$?
    # wait is interrupted by the trapped signal: repeat until dictate is gone
    while kill -0 "$rec" 2>/dev/null; do
      wait "$rec" || rc=$?
    done

    if ((rc != 0)); then
      notify-send -a dictate -u critical -r "$id" "Dictation failed" "$(tail -n1 "$tmp/err")"
      exit 1
    fi

    notify-send -a dictate -t 3000 -r "$id" "Dictation" "Typing…"
    wtype -- "$(<"$tmp/out")"
  '';
}
