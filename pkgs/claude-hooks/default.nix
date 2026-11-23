{
  lib,
  symlinkJoin,
  writeShellApplication,
  jq,
  jw,
  event-bus,
  coreutils,
  git,
  util-linux,
  findutils,
  gnugrep,
  gnused,
  python3,
  stdenvNoCC,
  makeWrapper,
}: let
  shellHook = {
    name,
    src,
    runtimeInputs,
    bashOptions ? [
      "nounset"
      "pipefail"
    ],
    # placeholder -> replacement, applied to the script text
    replacements ? {},
  }:
    writeShellApplication {
      inherit name runtimeInputs bashOptions;
      text = builtins.replaceStrings (builtins.attrNames replacements) (builtins.attrValues replacements) (
        builtins.readFile src
      );
    };

  guardWrites = shellHook {
    bashOptions = [
      "errexit"
      "nounset"
      "pipefail"
    ];
    name = "claude-guard-writes";
    src = ./guard-writes.sh;
    runtimeInputs = [
      jq
      coreutils
    ];
  };

  verifierGuard = shellHook {
    bashOptions = [
      "errexit"
      "nounset"
      "pipefail"
    ];
    name = "claude-verifier-guard";
    src = ./verifier-bash-guard.sh;
    runtimeInputs = [
      jq
      coreutils
      gnugrep
      gnused
    ];
  };

  sudoGuard = shellHook {
    bashOptions = [
      "errexit"
      "nounset"
      "pipefail"
    ];
    name = "claude-sudo-guard";
    src = ./sudo-guard.sh;
    runtimeInputs = [
      jq
      coreutils
      gnugrep
    ];
  };

  eventGuard = shellHook {
    bashOptions = [
      "errexit"
      "nounset"
      "pipefail"
    ];
    name = "claude-event-guard";
    src = ./event-guard.sh;
    runtimeInputs = [
      jq
      coreutils
    ];
  };

  # Deliberately without errexit: a failing git call must never fail the hook.
  architectAutocommit = shellHook {
    name = "claude-architect-autocommit";
    src = ./architect-autocommit.sh;
    runtimeInputs = [
      jq
      coreutils
      git
      util-linux
      findutils
    ];
  };

  # Publishes with event-pub. Never fails with exit 2 (blocking code).
  routeEvent = stdenvNoCC.mkDerivation {
    pname = "claude-route-event";
    version = "0.1.0";

    dontUnpack = true;
    nativeBuildInputs = [makeWrapper];

    doCheck = true;
    checkPhase = ''
      runHook preCheck
      cp ${./route_event.py} route_event.py
      cp ${./test_route_event.py} test_route_event.py
      ${python3}/bin/python3 -m unittest
      runHook postCheck
    '';

    installPhase = ''
      runHook preInstall
      install -Dm755 ${./route_event.py} $out/bin/claude-route-event
      substituteInPlace $out/bin/claude-route-event \
        --replace-fail "#!/usr/bin/env python3" "#!${python3}/bin/python3"
      wrapProgram $out/bin/claude-route-event --suffix PATH : ${
        lib.makeBinPath [
          jw
          event-bus
        ]
      }
      runHook postInstall
    '';

    meta.mainProgram = "claude-route-event";
  };

  # SUDO_ASKPASS helper: asks the user through agent-inbox, answer comes back on a FIFO.
  askpass = shellHook {
    name = "claude-askpass";
    src = ./askpass.sh;
    runtimeInputs = [
      jq
      coreutils
      routeEvent
    ];
  };

  # PermissionRequest hook: routes the request and waits for an answer from agent-inbox
  # (CLAUDE_INBOX_WAIT=<seconds>, default 600, 0 = off) and returns it as the decision.
  permissionHook = shellHook {
    name = "claude-permission-hook";
    src = ./permission-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      routeEvent
    ];
  };

  # Moves queued follow-up prompt files of an event session to read/ and prints their paths.
  eventQueue = shellHook {
    name = "claude-event-queue";
    src = ./event-queue.sh;
    runtimeInputs = [coreutils];
  };

  # Sets the status of the session record (busy, idle, waiting, ended); never fails.
  statusHook = shellHook {
    name = "claude-status-hook";
    src = ./status-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      jw
    ];
  };

  # Stop hook: points an event-started session at queued follow-up prompts (blocks the stop), else tells
  # the user it waits for them (claude.waiting) and sets the status to idle.
  stopHook = shellHook {
    name = "claude-stop-hook";
    src = ./stop-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      routeEvent
      eventQueue
      statusHook
    ];
  };

  # UserPromptSubmit hook: adds the paths of queued follow-up prompts of an event session as context.
  promptHook = shellHook {
    name = "claude-prompt-hook";
    src = ./prompt-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      eventQueue
    ];
  };

  # Seeds the project MCP servers of a directory as disabled (no blocking approval dialog); never fails.
  mcpDefaults = shellHook {
    name = "claude-mcp-defaults";
    src = ./mcp-defaults.sh;
    runtimeInputs = [
      jq
      coreutils
    ];
  };
in
  symlinkJoin {
    name = "claude-hooks";
    paths = [
      guardWrites
      verifierGuard
      sudoGuard
      eventGuard
      architectAutocommit
      routeEvent
      askpass
      permissionHook
      eventQueue
      statusHook
      stopHook
      promptHook
      mcpDefaults
    ];

    meta = {
      description = "Claude Code hook commands: write/bash guards, the event-session guard (no edits to AGENTS.md, CLAUDE.md or the knowledge file), architect memory autocommit, event routing, sudo askpass and the permission hook (answerable from agent-inbox), the Stop and UserPromptSubmit hooks (queued follow-up prompts of event-started sessions, waiting notification), the status hook (session record status), project MCP servers seeded as disabled";
      platforms = lib.platforms.unix;
    };
  }
