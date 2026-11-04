{
  lib,
  callPackage,
  writeShellApplication,
  symlinkJoin,
  gawk,
  coreutils,
  event-bus,
  # `<subject> <handler>` lines, see ./handlers
  handlers ? ./handlers,
}: let
  notify = callPackage ./notify {};

  set = {
    inherit notify;
    notify-critical = notify.override {urgency = "critical";};
    # GitLab event -> Claude session in a shpool session. No gitlab.* routes here (no
    # GitLab watcher): a layer that has one sets contextTool and adds the routes.
    claude = callPackage ./claude {inherit notify;};
    task = callPackage ./task {};
    # `session.message` events -> per-session mailbox read by the claude hooks
    session-message = callPackage ./session-message {};
    # queues the event for agent-inbox in `$XDG_RUNTIME_DIR/agent-inbox`
    inbox = callPackage ./inbox {};
    # `remote.notify.>` from the cluster (untrusted) -> notify, reduced to project/title/body
    remote-notify = callPackage ./remote-notify {inherit notify;};
  };

  # one event-subscribe per handler, with all its subjects
  runner = writeShellApplication {
    name = "event-handlers";
    runtimeInputs = [event-bus gawk coreutils] ++ lib.attrValues set;
    text = ''
      # jobs -p, not a list filled after each start: a TERM between a start and
      # recording its pid would leave that subscriber running
      stop() {
        local pids
        mapfile -t pids < <(jobs -p)
        kill "''${pids[@]}" 2>/dev/null || true
      }

      # event-subscribe's own trap only stops its `nats sub`, not a running handler
      trap 'stop; exit 0' TERM INT

      while read -r -a args; do
        event-subscribe "''${args[@]}" &
      done < <(awk '!/^[[:space:]]*(#|$)/ {s[$2] = s[$2] " " $1} END {for (h in s) print h s[h]}' ${handlers})

      wait -n || true
      echo "event-handlers: a subscriber exited, restarting all" >&2
      stop
      exit 1
    '';
  };
in
  symlinkJoin {
    name = "event-handlers";
    paths = [runner];
    passthru = set // {inherit handlers;};
    meta.mainProgram = "event-handlers";
    postBuild = ''
      # a bad handlers file fails the build
      awk -v q="'" '
        /^[[:space:]]*(#|$)/ {next}
        NF != 2 {printf "handlers:%d: expected %s<subject> <handler>%s\n", NR, q, q > "/dev/stderr"; bad = 1}
        {n++}
        END {
          if (!n) {print "handlers: no handlers" > "/dev/stderr"; bad = 1}
          exit bad
        }
      ' ${handlers}

      while read -r h; do
        PATH=${lib.makeBinPath (lib.attrValues set)} command -v "$h" >/dev/null || {
          echo "handlers: handler $h not found" >&2
          exit 1
        }
      done < <(awk '!/^[[:space:]]*(#|$)/ {print $2}' ${handlers} | sort -u)

      mkdir -p $out/lib/systemd/user
      substitute ${./event-handlers.service} $out/lib/systemd/user/event-handlers.service --subst-var out
    '';
  }
