{
  writeShellApplication,
  pipewire,
  curl,
  jq,
  coreutils,
}:
# Record from the default microphone until Enter, transcribe with the local
# whisper-server (host/features/optional/dictate.nix) and print the text on stdout.
# Enter instead of Ctrl-C: the output is usually piped into another program, and
# Ctrl-C would kill that too. SIGTERM/SIGINT to dictate also stop the recording.
writeShellApplication {
  name = "dictate";
  runtimeInputs = [
    pipewire
    curl
    jq
    coreutils
  ];
  text = ''
    whisper_url=''${DICTATE_WHISPER_URL:-http://127.0.0.1:8178}

    wav=$(mktemp --tmpdir --suffix=.wav dictate.XXXXXX)
    trap 'rm -f "$wav"' EXIT

    pw-record --rate 16000 --channels 1 --format s16 "$wav" &
    rec=$!

    # SIGTERM (background jobs of a non-interactive shell ignore SIGINT) makes
    # pw-record finalise the WAV header and exit.
    stop_rec() { kill -TERM "$rec" 2>/dev/null || true; }
    trap stop_rec INT TERM

    echo "Recording... press Enter to stop." >&2
    # /dev/tty may exist but not be openable (no controlling terminal)
    if { true </dev/tty; } 2>/dev/null; then
      # A backgrounded reader, because bash restarts a foreground read after a
      # trapped signal, so SIGTERM/SIGINT would not stop dictate.
      head -n1 </dev/tty >/dev/null &
      enter=$!
      wait -n "$rec" "$enter" || true # Enter, recorder exit, or trapped signal
      kill "$enter" 2>/dev/null || true
    else
      wait "$rec" || true
    fi
    stop_rec

    # wait is interrupted by trapped signals: repeat until the recorder is gone
    while kill -0 "$rec" 2>/dev/null; do
      wait "$rec" || true
    done
    # Recording is over: a later SIGTERM/SIGINT should cancel transcription
    trap - INT TERM

    # pw-record exits 1 even on a clean SIGTERM stop, so check the file instead:
    # anything beyond the 44-byte WAV header means audio was recorded
    if (($(stat -c %s "$wav") <= 44)); then
      echo "dictate: recording failed (no audio captured)" >&2
      exit 1
    fi

    echo "Transcribing..." >&2
    text=$(curl -sS --fail-with-body -F file=@"$wav" -F response_format=json "$whisper_url/inference" | jq -r 'if .error then ("dictate: server: \(.error)\n" | halt_error(1)) else .text // "" end')

    text=''${text//\[BLANK_AUDIO\]/}
    text=''${text#"''${text%%[![:space:]]*}"}
    text=''${text%"''${text##*[![:space:]]}"}

    if [[ -z "$text" ]]; then
      echo "dictate: no speech recognised" >&2
      exit 1
    fi
    printf '%s\n' "$text"
  '';
}
