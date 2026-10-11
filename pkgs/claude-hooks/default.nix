{
  lib,
  symlinkJoin,
  writeShellApplication,
  jq,
  curl,
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
  # Repos that code-mode agents may only edit when their cwd is inside them.
  # Entries may contain a literal $HOME, expanded by the script at runtime (not by Nix).
  protectedRepos ? [
    "$HOME/projects/real-dotfiles"
    "$HOME/projects/dotfiles"
  ],
}:

# an entry like ~/x or ${HOME}/x would resolve against the agent's cwd and protect nothing
assert lib.assertMsg (lib.all (r: lib.hasPrefix "/" r || lib.hasPrefix "$HOME/" r) protectedRepos)
  "claude-hooks: protectedRepos entries must start with / or $HOME/";

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

  # Shell word with everything single-quoted except a literal $HOME, which is left to expand at runtime
  quoteKeepingHome =
    s:
    lib.concatStringsSep ''"$HOME"'' (
      map (part: lib.optionalString (part != "") (lib.escapeShellArg part)) (lib.splitString "$HOME" s)
    );

  guardWrites = shellHook {
    name = "claude-guard-writes";
    src = ./guard-writes.sh;
    runtimeInputs = [
      jq
      coreutils
    ];
    replacements."@protectedRepos@" = lib.concatMapStringsSep " " quoteKeepingHome protectedRepos;
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
  # (event-handlers.knowledge -> claude -> claude-hooks). Never fails with exit 2 (blocking code).
  routeEvent = shellHook {
    name = "claude-route-event";
    src = ./route-event.sh;
    runtimeInputs = [
      jq
      curl
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
    jjWorkspace
  ];

  meta = {
    description = "Claude Code hook commands: write/bash guards, architect memory autocommit, event routing and sudo askpass, jj workspaces for isolated agents";
    platforms = lib.platforms.unix;
  };
}
