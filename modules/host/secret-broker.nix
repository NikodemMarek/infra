{
  pkgs,
  lib,
  config,
  ...
}: let
  cfg = config.services.secret-broker;
  pkg = pkgs.secret-broker;

  reservedEnv = ["PATH" "HOME" "TMPDIR" "LANG" "SSL_CERT_FILE" "NIX_SSL_CERT_FILE"];

  configFile = pkgs.writeText "secret-broker.json" (builtins.toJSON {
    inherit (cfg) user lockAfter exportProvider exportProfiles bwServer;
    actions = lib.mapAttrs (_: a: {inherit (a) description command args secrets timeout;}) cfg.actions;
  });

  # a bad config fails the build, not the service start
  checkedConfig = pkgs.runCommand "secret-broker-config.json" {} ''
    ${pkg}/bin/secret-broker check ${configFile}
    cp ${configFile} $out
  '';

  tty = "/dev/tty9";
  coreutils = "${pkgs.coreutils}/bin";
in {
  options.services.secret-broker = {
    enable = lib.mkEnableOption "the secret broker for agents";
    user = lib.mkOption {
      type = lib.types.str;
      description = "The only user whose processes may talk to the broker";
    };
    lockAfter = lib.mkOption {
      type = lib.types.ints.between 1 1440;
      default = 240;
      description = "Minutes idle until the vault is locked again";
    };
    exportProvider = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Bitwarden provider URI for `secret export`, null disables export";
    };
    exportProfiles = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "secretspec profiles `secret export` may return";
    };
    bwServer = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Bitwarden server URL the CLI must be logged in to, null for the default server";
    };
    actions = lib.mkOption {
      default = {};
      description = "Commands agents may run with secrets, after approval on the console";
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          description = lib.mkOption {type = lib.types.str;};
          command = lib.mkOption {type = lib.types.nonEmptyListOf lib.types.str;};
          args = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = "One full-match regex per argument";
          };
          secrets = lib.mkOption {
            type = lib.types.attrsOf lib.types.str;
            default = {};
            description = "Environment variable name to exact Bitwarden item name";
          };
          timeout = lib.mkOption {
            type = lib.types.ints.between 1 3600;
            default = 120;
            description = "Seconds";
          };
        };
      });
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = lib.concatLists (lib.mapAttrsToList (name: action: [
        {
          assertion = builtins.match "[a-z0-9][a-z0-9-]*" name != null && !(lib.elem name ["list" "export"]);
          message = "services.secret-broker.actions.${name}: the name must match [a-z0-9][a-z0-9-]* and not be list or export.";
        }
        {
          assertion = let
            cmd = lib.head action.command;
          in
            lib.hasPrefix "/nix/store/" cmd && !(lib.elem ".." (lib.splitString "/" cmd));
          message = "services.secret-broker.actions.${name}: command[0] must be a /nix/store/ path without `..`.";
        }
        {
          assertion = lib.all (env: builtins.match "[A-Z_][A-Z0-9_]*" env != null && !(lib.elem env reservedEnv)) (lib.attrNames action.secrets);
          message = "services.secret-broker.actions.${name}: secret environment names must match [A-Z_][A-Z0-9_]* and not be one of ${lib.concatStringsSep ", " reservedEnv}.";
        }
      ])
      cfg.actions);

    users.users.secret-broker = {
      isSystemUser = true;
      group = "secret-broker";
    };
    users.groups.secret-broker = {};

    environment.systemPackages = [pkg];

    persist.generated.directories = lib.mkIf config.persist.enable [
      {
        directory = "/var/lib/secret-broker";
        user = "secret-broker";
        group = "secret-broker";
        mode = "0700";
      }
    ];

    boot.kernel.sysctl = {
      # the default 16 (sync) plus 4 (keyboard control: Alt+SysRq+R takes the keyboard from the X server)
      "kernel.sysrq" = 20;
      "kernel.yama.ptrace_scope" = lib.mkDefault 1;
    };

    systemd.sockets.secret-broker = {
      description = "Secret broker socket";
      wantedBy = ["sockets.target"];
      listenStreams = ["/run/secret-broker.sock"];
      socketConfig = {
        SocketMode = "0660";
        SocketUser = "root";
        SocketGroup = config.users.users.${cfg.user}.group;
      };
    };

    systemd.services.secret-broker = {
      description = "Secret broker for agents";
      requires = ["secret-broker.socket"];
      after = ["secret-broker.socket" "systemd-user-sessions.service"];
      conflicts = ["getty@tty9.service"];
      wantedBy = ["multi-user.target"];
      serviceConfig = {
        ExecStartPre = [
          "+${coreutils}/chown secret-broker:tty ${tty}"
          "+${coreutils}/chmod 0600 ${tty}"
        ];
        ExecStart = "${pkg}/bin/secret-broker serve ${checkedConfig}";
        User = "secret-broker";
        Group = "secret-broker";
        Restart = "always";
        RestartSec = 2;
        StateDirectory = "secret-broker";
        StateDirectoryMode = "0700";
        Environment = ["BITWARDENCLI_APPDATA_DIR=%S/secret-broker/bw"];

        TTYPath = tty;
        StandardInput = "tty";
        StandardOutput = "tty";
        StandardError = "journal";
        TTYReset = true;
        TTYVHangup = true;
        TTYVTDisallocate = true;

        # the action runner relies on the service's cgroup being swept clean
        TasksMax = 64;
        LimitCORE = 0;
        MemorySwapMax = 0;
        UMask = "0077";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ProtectProc = "invisible";
        ProcSubset = "pid";
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        LockPersonality = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        RestrictNamespaces = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = ["@system-service"];
        RestrictAddressFamilies = ["AF_UNIX" "AF_INET" "AF_INET6"];
        DevicePolicy = "closed";
        DeviceAllow = ["${tty} rw"];
      };
    };
  };
}
