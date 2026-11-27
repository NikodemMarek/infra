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
}: let
  shellHook = {
    name,
    src,
    runtimeInputs,
    bashOptions ? [
      "nounset"
      "pipefail"
    ],
  }:
    writeShellApplication {
      inherit name runtimeInputs bashOptions;
      text = builtins.readFile src;
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

  # Sets the status of the session record (busy, idle, waiting); never fails.
  statusHook = shellHook {
    name = "claude-status-hook";
    src = ./status-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      jw
    ];
  };

  # Stop hook: sets the status to idle and posts claude.waiting for the supervisor; never fails.
  stopHook = shellHook {
    name = "claude-stop-hook";
    src = ./stop-hook.sh;
    runtimeInputs = [
      jq
      coreutils
      event-bus
      statusHook
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
      architectAutocommit
      statusHook
      stopHook
      mcpDefaults
    ];

    meta = {
      description = "Claude Code hook commands: write/bash guards, architect memory autocommit, the Stop hook (status idle and claude.waiting event), the status hook (session record status), project MCP servers seeded as disabled";
      platforms = lib.platforms.unix;
    };
  }
