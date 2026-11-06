{
  lib,
  callPackage,
  dockerTools,
  buildEnv,
  runCommand,
  symlinkJoin,
  makeWrapper,
  writeText,
  writeShellApplication,
  writeShellScriptBin,
  claude,
  jw,
  event-handlers,
  jujutsu,
  gh,
  git,
  natscli,
  tini,
  shpool,
  bashInteractive,
  coreutils,
  findutils,
  gnugrep,
  gnused,
  jq,
  curl,
  cacert,
  ncurses,
  less,
  ripgrep,
  procps,
}: let
  forwardNotify = callPackage ./forward-notify.nix {};
  stopFailureHook = callPackage ./stop-failure-hook.nix {};

  # no devenv/nix in the pod: jw runs `direnv exec . CMD...`
  direnvShim = writeShellScriptBin "direnv" ''
    [ "$1" = exec ] || { echo "direnv shim: only 'exec DIR CMD...'" >&2; exit 2; }
    shift 2
    exec "$@"
  '';

  shaniJw = jw.override {
    projectsRoot = "/projects";
    direnv = direnvShim;
  };

  sessionMessage = callPackage ./session-message.nix {
    jw = shaniJw;
    forward-notify = forwardNotify;
  };

  shaniHandlers = callPackage ./handlers.nix {
    forward-notify = forwardNotify;
    session-message = sessionMessage;
  };

  shaniEventHandlers = event-handlers.override {handlers = shaniHandlers;};

  shaniClaude = claude.extend {
    settings = {
      permissions = {
        deny = ["AskUserQuestion"];
        additionalDirectories = ["/projects"];
      };
      hooks.StopFailure = [
        {
          hooks = [
            {
              type = "command";
              command = "${lib.getExe stopFailureHook} || true";
              timeout = 10;
            }
          ];
        }
      ];
    };
  };

  shaniGh = symlinkJoin {
    name = "gh";
    paths = [gh];
    nativeBuildInputs = [makeWrapper];
    postBuild = ''
      wrapProgram $out/bin/gh --run '[ ! -r /secrets/gh_token ] || export GH_TOKEN=$(< /secrets/gh_token)'
    '';
  };

  gitConfig = writeText "gitconfig" ''
    [credential]
      helper = !gh auth git-credential
  '';

  shpoolConfig = writeText "shpool.toml" ''
    prompt_prefix = ""
  '';

  entrypoint = writeShellApplication {
    name = "shani-entrypoint";
    runtimeInputs = [shaniEventHandlers shaniJw forwardNotify];
    text = ''
      socket=$XDG_RUNTIME_DIR/shpool/shpool.socket
      rm -f "$socket"
      shpool --config-file ${shpoolConfig} daemon &
      shpool_pid=$!

      until [[ -S $socket ]]; do
        if ! kill -0 "$shpool_pid" 2>/dev/null; then
          echo "shani: shpool exited during startup" >&2
          exit 1
        fi
        sleep 0.2
      done

      ER_TYPE=shani.started event-handler-forward-notify <<<'{"title":"shani started"}' || true

      while true; do
        event-handlers || true
        echo "shani: event-handlers exited, restarting" >&2
        sleep 2
      done &

      while sleep 600; do
        timeout 15m jw reap || true
        timeout 15m jw prune --yes || true
      done &

      wait "$shpool_pid" || true
      echo "shani: shpool exited" >&2
      exit 1
    '';
  };

  etc = runCommand "shani-etc" {} ''
    mkdir -p $out/etc
    echo 'root:x:0:0:root:/root:/bin/bash' > $out/etc/passwd
    echo 'shani:x:1000:100:shani:/home/shani:/bin/bash' >> $out/etc/passwd
    echo 'root:x:0:' > $out/etc/group
    echo 'users:x:100:' >> $out/etc/group
  '';

  # only /bin is linked, so nothing else from the packages lands in the image
  tools = buildEnv {
    name = "shani-tools";
    pathsToLink = ["/bin"];
    paths = [
      shaniClaude
      shaniJw
      jujutsu
      shaniGh
      git
      natscli
      tini
      shpool
      bashInteractive
      coreutils
      findutils
      gnugrep
      gnused
      jq
      curl
      ncurses
      less
      ripgrep
      procps
    ];
  };
in
  dockerTools.buildLayeredImage {
    name = "shani";
    contents = [tools etc];
    compressor = "zstd";
    config = {
      User = "1000:100";
      Entrypoint = ["${tini}/bin/tini" "-g" "--" (lib.getExe entrypoint)];
      Env = [
        "PATH=/bin"
        "HOME=/home/shani"
        "XDG_RUNTIME_DIR=/tmp"
        "LANG=C.UTF-8"
        "SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt"
        "TERMINFO_DIRS=${ncurses}/share/terminfo"
        "GIT_CONFIG_SYSTEM=${gitConfig}"
        "NATS_URL=nats://nats.nats.svc.cluster.local:4222"
      ];
    };
  }
