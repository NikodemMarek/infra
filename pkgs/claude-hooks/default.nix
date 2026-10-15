{
  lib,
  symlinkJoin,
  writeShellApplication,
  jq,
  curl,
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

  # Posts to the event router with curl: depending on pkgs.event-router would be a dependency cycle
  # (event-router -> handlers -> claude -> claude-hooks). Never fails with exit 2 (blocking code).
  routeEvent = shellHook {
    name = "claude-route-event";
    src = ./route-event.sh;
    runtimeInputs = [
      jq
      curl
      jw
      coreutils
    ];
    bashOptions = [
      "nounset"
      "pipefail"
    ];
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

  # Stop hook: points an event-started session at queued follow-up prompts (blocks the stop), else tells
  # the user it waits for them (claude.waiting).
  stopHook = shellHook {
    name = "claude-stop-hook";
    src = ./stop-hook.sh;
    runtimeInputs = [
      jq
      routeEvent
      eventQueue
    ];
    bashOptions = [
      "nounset"
      "pipefail"
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
    stopHook
    promptHook
    jjWorkspace
  ];

  meta = {
    description = "Claude Code hook commands: write/bash guards, architect memory autocommit, event routing, sudo askpass and the permission hook (answerable from agent-inbox), the Stop and UserPromptSubmit hooks of event-started sessions (queued follow-up prompts, waiting notification), jj workspaces for isolated agents";
    platforms = lib.platforms.unix;
  };
}
