{
  lib,
  symlinkJoin,
  writeShellApplication,
  jq,
  jw,
  coreutils,
  git,
  util-linux,
  findutils,
  gnugrep,
  gnused,
  python3,
  jujutsu,
  stdenvNoCC,
  makeWrapper,
}:

let
  shellHook =
    {
      name,
      src,
      runtimeInputs,
      bashOptions ? null,
      # placeholder -> replacement, applied to the script text
      replacements ? { },
    }:
    writeShellApplication (
      {
        inherit name runtimeInputs;
        text = builtins.replaceStrings (builtins.attrNames replacements) (builtins.attrValues replacements) (
          builtins.readFile src
        );
      }
      // lib.optionalAttrs (bashOptions != null) { inherit bashOptions; }
    );

  guardWrites = shellHook {
    name = "claude-guard-writes";
    src = ./guard-writes.sh;
    runtimeInputs = [
      jq
      coreutils
    ];
  };

  verifierGuard = shellHook {
    name = "claude-verifier-guard";
    src = ./verifier-bash-guard.sh;
    runtimeInputs = [
      jq
      coreutils
      gnugrep
      gnused
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
    bashOptions = [
      "nounset"
      "pipefail"
    ];
  };

  # Posts to the event router over plain HTTP: depending on pkgs.event-router would be a dependency cycle
  # (event-router -> handlers -> claude -> claude-hooks). Never fails with exit 2 (blocking code).
  routeEvent = stdenvNoCC.mkDerivation {
    pname = "claude-route-event";
    version = "0.1.0";

    dontUnpack = true;
    nativeBuildInputs = [ makeWrapper ];

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
      wrapProgram $out/bin/claude-route-event --suffix PATH : ${lib.makeBinPath [ jw ]}
      runHook postInstall
    '';

    meta.mainProgram = "claude-route-event";
  };

  # SUDO_ASKPASS helper: asks the user through the event router, answer comes back on a FIFO.
  askpass = shellHook {
    name = "claude-askpass";
    src = ./askpass.sh;
    runtimeInputs = [
      jq
      coreutils
      routeEvent
    ];
    bashOptions = [
      "nounset"
      "pipefail"
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
    bashOptions = [
      "nounset"
      "pipefail"
    ];
  };

  # Moves queued follow-up prompt files of an event session to read/ and prints their paths.
  eventQueue = shellHook {
    name = "claude-event-queue";
    src = ./event-queue.sh;
    runtimeInputs = [ coreutils ];
    bashOptions = [
      "nounset"
      "pipefail"
    ];
  };

  # Prints the session messages queued for this session (mailbox of the session.message event handler)
  # and moves them to read/.
  mailTake = shellHook {
    name = "claude-mail-take";
    src = ./mail-take.sh;
    runtimeInputs = [
      coreutils
      eventQueue
    ];
    bashOptions = [
      "nounset"
      "pipefail"
    ];
  };

  # Stop hook: points an event-started session at queued follow-up prompts and hands it queued session
  # messages (blocks the stop), else tells the user it waits for them (claude.waiting).
  stopHook = shellHook {
    name = "claude-stop-hook";
    src = ./stop-hook.sh;
    runtimeInputs = [
      jq
      routeEvent
      eventQueue
      mailTake
    ];
    bashOptions = [
      "nounset"
      "pipefail"
    ];
  };

  # UserPromptSubmit hook: adds the paths of queued follow-up prompts of an event session and queued
  # session messages as context.
  promptHook = shellHook {
    name = "claude-prompt-hook";
    src = ./prompt-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      eventQueue
      mailTake
    ];
    bashOptions = [
      "nounset"
      "pipefail"
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
    bashOptions = [
      "nounset"
      "pipefail"
    ];
  };

  jjWorkspace = stdenvNoCC.mkDerivation {
    pname = "claude-jj-workspace";
    version = "0.1.0";

    dontUnpack = true;
    nativeBuildInputs = [ makeWrapper ];

    installPhase = ''
      runHook preInstall
      install -Dm755 ${./jj-workspace.py} $out/bin/claude-jj-workspace
      substituteInPlace $out/bin/claude-jj-workspace \
        --replace-fail "#!/usr/bin/env python3" "#!${python3}/bin/python3"
      wrapProgram $out/bin/claude-jj-workspace \
        --suffix PATH : ${
          lib.makeBinPath [
            jujutsu
            git
            mcpDefaults
          ]
        }
      runHook postInstall
    '';

    meta.mainProgram = "claude-jj-workspace";
  };
in
symlinkJoin {
  name = "claude-hooks";
  paths = [
    guardWrites
    verifierGuard
    architectAutocommit
    routeEvent
    askpass
    permissionHook
    eventQueue
    mailTake
    stopHook
    promptHook
    mcpDefaults
    jjWorkspace
  ];

  meta = {
    description = "Claude Code hook commands: write/bash guards, architect memory autocommit, event routing, sudo askpass and the permission hook (answerable from agent-inbox), the Stop and UserPromptSubmit hooks (queued follow-up prompts of event-started sessions, queued session messages, waiting notification), jj workspaces for isolated agents, project MCP servers seeded as disabled";
    platforms = lib.platforms.unix;
  };
}
