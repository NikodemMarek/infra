{
  writeShellApplication,
  rofi,
  wl-clipboard,
  grim,
  slurp,
  curl,
  jq,
  libnotify,
  coreutils,
  findutils,
}:
# Quick capture of a task for the event router (ADR-035): a rofi prompt, optionally after a
# slurp screenshot (--shot) and with the clipboard image attached on Ctrl+Enter. Posts
# `task.new` and exits; the images stay in $XDG_RUNTIME_DIR/agent-task/ for the session.
writeShellApplication {
  name = "agent-task";
  runtimeInputs = [
    rofi
    wl-clipboard
    grim
    slurp
    curl
    jq
    libnotify
    coreutils
    findutils
  ];
  text = ''
    shot=0
    case $# in
      0) ;;
      1) if [[ $1 == --shot ]]; then shot=1; else shot=usage; fi ;;
      *) shot=usage ;;
    esac
    if [[ $shot == usage ]]; then
      echo "usage: agent-task [--shot]" >&2
      exit 2
    fi

    if [[ -z "''${XDG_RUNTIME_DIR:-}" ]]; then
      echo "agent-task: XDG_RUNTIME_DIR is not set" >&2
      exit 1
    fi
    root=$XDG_RUNTIME_DIR/agent-task
    mkdir -p "$root"
    chmod 700 "$root"
    find "$root" -mindepth 1 -maxdepth 1 -type d -mmin +2880 -exec rm -rf {} +
    dir=$(mktemp -d -p "$root")

    if ((shot)); then
      geom=$(slurp) || {
        rm -rf "$dir"
        exit 0
      }
      grim -g "$geom" "$dir/shot.png"
    fi

    hints=()
    if [[ -e $dir/shot.png ]]; then
      hints+=("screenshot attached")
    fi
    types=$(wl-paste --list-types 2>/dev/null || true)
    if [[ $'\n'$types$'\n' == *$'\nimage/png\n'* ]]; then
      hints+=("Ctrl+Enter: attach clipboard image")
    fi
    mesg=()
    if ((''${#hints[@]})); then
      printf -v hint '%s | ' "''${hints[@]}"
      mesg=(-mesg "''${hint% | }")
    fi

    rc=0
    text=$(rofi -dmenu -p task -l 0 -kb-accept-custom "" -kb-custom-1 Control+Return "''${mesg[@]}" </dev/null) || rc=$?
    case $rc in
      0) ;;
      10)
        wl-paste --type image/png >"$dir/clip.png" || true
        [[ -s $dir/clip.png ]] || rm -f "$dir/clip.png"
        ;;
      *)
        rm -rf "$dir"
        exit 0
        ;;
    esac

    text=''${text#"''${text%%[![:space:]]*}"}
    text=''${text%"''${text##*[![:space:]]}"}

    mapfile -d "" -t images < <(find "$dir" -maxdepth 1 -type f -name '*.png' -print0 | sort -z)
    if [[ -z $text ]] && ((''${#images[@]} == 0)); then
      rm -rf "$dir"
      exit 0
    fi

    if ! err=$(jq -n --arg prompt "$text" '
          {type: "task.new"}
          + (if $prompt == "" then {} else {prompt: $prompt} end)
          + {images: $ARGS.positional}' --args "''${images[@]}" |
      curl -fsS --noproxy '*' -m 5 -H 'Content-Type: application/json' --data-binary @- \
        "''${EVENT_ROUTER_URL:-http://127.0.0.1:7531}/events" 2>&1 >/dev/null); then
      notify-send -u critical "Task not sent" "$err"
      exit 1
    fi
    ((''${#images[@]})) || rmdir "$dir"
  '';
}
