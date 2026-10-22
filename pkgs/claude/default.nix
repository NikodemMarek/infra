{
  lib,
  claude-code,
  symlinkJoin,
  makeWrapper,
  runCommand,
  writeText,
  writeShellScript,
  coreutils,
  findutils,
  jq,
  claude-hooks,
  claude-statusline,
  jj-upload,
  knowledge,
  agent-skills,
  # CLAUDE.md (path). null -> ./CLAUDE.md
  instructions ? null,
  # name (without .md) -> path. null -> every .md in ./agents
  agents ? null,
  # name -> directory path. null -> every skill in agent-skills (shared with other tools)
  skills ? null,
  # User settings as a Nix attrset, linked read-only to
  # $CLAUDE_CONFIG_DIR/settings.json. null -> ./settings.nix
  settings ? null,
  # extra packages put on PATH of the wrapped claude
  extraPackages ? [ ],
  # where claude keeps its state/config; shell-expanded at runtime
  dataDir ? "$HOME/.local/share/claude",
}:

let
  mdEntries =
    dir: lib.filterAttrs (n: t: t == "regular" && lib.hasSuffix ".md" n) (builtins.readDir dir);

  defaultAgents = lib.mapAttrs' (
    n: _: lib.nameValuePair (lib.removeSuffix ".md" n) (./agents + "/${n}")
  ) (mdEntries ./agents);

  # names come from the source (no IFD), the paths from the package
  defaultSkills = lib.mapAttrs (n: _: "${agent-skills}/${n}") (
    lib.filterAttrs (_: t: t == "directory") (builtins.readDir ../agent-skills/skills)
  );

  # attrsets recurse, lists concatenate (deduped), anything else: b wins
  deepMerge =
    a: b:
    if builtins.isAttrs a && builtins.isAttrs b && !(lib.isDerivation a) && !(lib.isDerivation b) then
      lib.zipAttrsWith
        (
          _: vs:
          if builtins.length vs == 1 then
            builtins.head vs
          else
            deepMerge (builtins.elemAt vs 0) (builtins.elemAt vs 1)
        )
        [
          a
          b
        ]
    else if builtins.isList a && builtins.isList b then
      lib.unique (a ++ b)
    else
      b;

  dropNulls = lib.filterAttrs (_: v: v != null);

  knownKeys = [
    "instructions"
    "agents"
    "skills"
    "settings"
    "extraPackages"
    "dataDir"
  ];

  # Inner builder: takes fully-resolved config. Wrapped in makeOverridable so
  # results of `extend` still support `.override`, and recursion through
  # `build` makes `extend` chain.
  build = lib.makeOverridable (
    {
      instructions,
      agents,
      skills,
      settings,
      extraPackages,
      dataDir,
    }:
    let
      # "$HOME/.local/share/claude" -> "~/.local/share/claude" (for prose in the markdown files)
      # The deny rules in the settings use the `~/` form, so a dataDir outside $HOME would silently disable them.
      prettyDataDir =
        assert lib.assertMsg (lib.hasPrefix "$HOME/" dataDir) "claude: dataDir must start with $HOME/ (got ${dataDir})";
        "~" + lib.removePrefix "$HOME" dataDir;
      prettyMemoryDir = "${prettyDataDir}/memory";

      settingsFile = writeText "claude-settings.json" (builtins.toJSON settings);

      config = runCommand "claude-config" { } ''
        mkdir -p "$out/agents" "$out/skills"
        cp ${instructions} "$out/CLAUDE.md"
        ${lib.concatStringsSep "\n" (lib.mapAttrsToList (n: p: ''cp "${p}" "$out/agents/${n}.md"'') agents)}
        ${lib.concatStringsSep "\n" (
          lib.mapAttrsToList (n: p: ''cp -rL "${p}" "$out/skills/${n}"'') skills
        )}
        cp ${settingsFile} "$out/settings.json"
        chmod -R u+w "$out"

        find "$out" -type f \( -name '*.md' -o -name 'settings.json' \) -exec sed -i \
          -e ${lib.escapeShellArg "s|@dataDir@|${prettyDataDir}|g"} \
          -e ${lib.escapeShellArg "s|@memoryDir@|${prettyMemoryDir}|g"} \
          {} +
      '';

      # Everything the launch preamble keeps linked into $CLAUDE_CONFIG_DIR.
      entries = [
        "CLAUDE.md"
        "settings.json"
      ]
      ++ map (n: "agents/${n}.md") (lib.attrNames agents)
      ++ map (n: "skills/${n}") (lib.attrNames skills);

      managed = "${builtins.storeDir}/*-claude-config/*";

      # Sourced from the makeWrapper script, which runs with `bash -e`: all work
      # happens in a function called through `|| true` (this disables -e inside
      # it), and nothing but the two exports is left behind.
      preamble = writeShellScript "claude-preamble" ''
        __claude_link() {
          local dst="$CLAUDE_CONFIG_DIR/$1" src="$2" cur
          if [ -L "$dst" ]; then
            cur="$(${coreutils}/bin/readlink "$dst")"
            if [ "$cur" = "$src" ]; then
              return 0
            fi
            case "$cur" in
              ${managed})
                # atomic replace: never leaves $dst missing for a concurrent launch
                ${coreutils}/bin/ln -s "$src" "$dst.tmp.$$" \
                  && ${coreutils}/bin/mv -T "$dst.tmp.$$" "$dst" \
                  || ${coreutils}/bin/rm -f "$dst.tmp.$$"
                ;;
              *)
                echo "claude: not replacing $dst (not managed by nix)" >&2
                ;;
            esac
          elif [ -e "$dst" ]; then
            echo "claude: not replacing $dst (not managed by nix)" >&2
          elif ! ${coreutils}/bin/ln -s "$src" "$dst" 2>/dev/null; then
            # a concurrent launch may have created it in the meantime
            [ "$(${coreutils}/bin/readlink "$dst" 2>/dev/null)" = "$src" ] \
              || echo "claude: could not link $dst" >&2
          fi
        }

        __claude_prune() {
          local dir l t
          for dir in "$CLAUDE_CONFIG_DIR" "$CLAUDE_CONFIG_DIR/agents" "$CLAUDE_CONFIG_DIR/skills"; do
            for l in "$dir"/*; do
              [ -L "$l" ] || continue
              t="$(${coreutils}/bin/readlink "$l")"
              case "$t" in
                "${config}"/*) ;;
                ${managed}) ${coreutils}/bin/rm -f "$l" ;;
              esac
            done
          done
        }

        # settings.json is claude's to write by default; move a real file (or a
        # foreign link) aside once so the managed link can take its place.
        __claude_adopt_settings() {
          local dst="$CLAUDE_CONFIG_DIR/settings.json" bak
          if [ -L "$dst" ]; then
            case "$(${coreutils}/bin/readlink "$dst")" in
              ${managed}) return 0 ;;
            esac
          elif [ ! -e "$dst" ]; then
            return 0
          fi
          local n=0 stamp
          stamp="backups/settings.json.$(${coreutils}/bin/date +%Y%m%d-%H%M%S)"
          bak="$stamp"
          # never clobber an earlier backup from the same second
          while [ -e "$CLAUDE_CONFIG_DIR/$bak" ] || [ -L "$CLAUDE_CONFIG_DIR/$bak" ]; do
            n=$((n + 1))
            bak="$stamp.$n"
          done
          ${coreutils}/bin/mkdir -p "$CLAUDE_CONFIG_DIR/backups" \
            && ${coreutils}/bin/mv -T "$dst" "$CLAUDE_CONFIG_DIR/$bak" \
            && echo "claude: moved unmanaged settings.json to $bak" >&2
        }

        __claude_setup() {
          export CLAUDE_CONFIG_DIR="''${CLAUDE_CONFIG_DIR:-${dataDir}}"
          # unconditional: the agents' prose has this path baked in at build time
          export CLAUDE_MEMORY_DIR="$CLAUDE_CONFIG_DIR/memory"
          ${coreutils}/bin/mkdir -p "$CLAUDE_CONFIG_DIR/agents" "$CLAUDE_CONFIG_DIR/skills" "$CLAUDE_MEMORY_DIR"
          if [ ! -e "$CLAUDE_MEMORY_DIR/.git" ] && command -v git >/dev/null 2>&1; then
            git -C "$CLAUDE_MEMORY_DIR" init -q >/dev/null 2>&1
          fi
          local nested
          nested="$(${findutils}/bin/find "$CLAUDE_MEMORY_DIR" -mindepth 2 -maxdepth 3 -name .git -print -quit 2>/dev/null)"
          if [ -n "$nested" ]; then
            echo "claude: $CLAUDE_MEMORY_DIR contains a nested git repository ($nested); memory commits miss files below it" >&2
          fi
          __claude_adopt_settings
          ${lib.concatMapStringsSep "\n" (
            e: ''__claude_link ${lib.escapeShellArg e} "${config}/${e}"''
          ) entries}
          __claude_prune
        }

        __claude_setup || true
        unset -f __claude_link __claude_prune __claude_adopt_settings __claude_setup
      '';

      # Sourced after the preamble: a plain interactive `claude [prompt]` in a repo's main jj
      # workspace runs in its own one (`--worktree`, see claude-jj-workspace). Anything with
      # flags or a subcommand, or already in a <repo>.agents/<name> workspace, runs as is.
      # Exception: the human's <repo>.agents/w-* workspaces (`jw new`) are never worked in by
      # claude directly; there it starts from the main workspace (the one whose trust counts)
      # and the session workspace begins on the w-* workspace's current change
      # (CLAUDE_WORKSPACE_BASE, read by claude-jj-workspace).
      # CLAUDE_NO_WORKSPACE=1 opts out. Always (unless already set) exports
      # CLAUDE_CODE_PROJECT_DIR_NAME as the main repo's session key, so all of a repo's
      # workspaces share one session list (`--resume`).
      workspace = writeShellScript "claude-workspace" ''
        __claude_ws=""
        __claude_root="$(jj --ignore-working-copy workspace root 2>/dev/null)" || true
        __claude_project() {
          [ -z "''${CLAUDE_CODE_PROJECT_DIR_NAME:-}" ] && [ -n "$__claude_root" ] || return 0
          local main="$__claude_root" key sum
          case "$(${coreutils}/bin/dirname "$main")" in
            ?*.agents) main="$(${coreutils}/bin/dirname "$main")"; main="''${main%.agents}" ;;
          esac
          # claude's default key, but it must fit in 64 chars: keep the tail plus a checksum
          key="''${main//[^A-Za-z0-9]/-}"
          if [ "''${#key}" -gt 64 ]; then
            sum="$(printf %s "$main" | ${coreutils}/bin/cksum)"
            key="''${key: -50}-''${sum%% *}"
          fi
          export CLAUDE_CODE_PROJECT_DIR_NAME="$key"
        }
        __claude_project || true
        __claude_workspace() {
          [ -t 0 ] && [ -t 1 ] && [ -z "''${CLAUDE_NO_WORKSPACE:-}" ] || return 0
          local a
          for a in "$@"; do
            case "$a" in -*) return 0 ;; esac
          done
          case "''${1:-}" in
            agents|attach|auth|auto-mode|doctor|gateway|import|install|logs|mcp|plugin|plugins|purge|respawn|rm|setup-token|stop|kill|ultrareview|update|upgrade) return 0 ;;
          esac
          [ -n "$__claude_root" ] || return 0
          local main="$__claude_root" base=""
          case "$(${coreutils}/bin/dirname "$__claude_root")" in
            ?*.agents)
              # only the human's w-* workspaces get a session workspace; a-*, s-* etc. run as is
              case "$(${coreutils}/bin/basename "$__claude_root")" in w-*) ;; *) return 0 ;; esac
              main="$(${coreutils}/bin/dirname "$__claude_root")"; main="''${main%.agents}"
              ;;
          esac
          # claude refuses --worktree until this repo itself is trusted (a trusted parent dir doesn't count)
          if ! ${jq}/bin/jq -e --arg p "$main" '.projects[$p].hasTrustDialogAccepted == true' \
            "$CLAUDE_CONFIG_DIR/.claude.json" >/dev/null 2>&1; then
            echo "claude: $main is not trusted yet; running here without a workspace this once" >&2
            return 0
          fi
          if [ "$main" != "$__claude_root" ]; then
            # snapshots the w-* workspace, so unsaved edits are part of the base change
            base="$(jj -R "$__claude_root" log --no-graph -r @ -T change_id 2>/dev/null)" || return 0
            [ -n "$base" ] || return 0
            cd "$main" || return 0
            export CLAUDE_WORKSPACE_BASE="$base"
          fi
          # the jj workspace is renamed to s-<title> by `claude-jj-workspace title`; the directory keeps this name
          __claude_ws="s-$(${coreutils}/bin/date +%m%d-%H%M%S)-$RANDOM"
        }
        __claude_workspace "$@" || true
        if [ -n "$__claude_ws" ]; then
          set -- --worktree "$__claude_ws" "$@"
        elif [ -t 0 ] && [ -t 1 ]; then
          # project MCP servers start disabled: avoids the blocking approval dialog (a --worktree
          # session gets the same from claude-jj-workspace when it creates the workspace)
          ${claude-hooks}/bin/claude-mcp-defaults "$PWD" || true
        fi
        unset -f __claude_project __claude_workspace
        unset __claude_ws __claude_root
      '';
    in
    symlinkJoin {
      name = "claude-${claude-code.version}";
      paths = [ claude-code ];
      nativeBuildInputs = [ makeWrapper ];
      postBuild = ''
        wrapProgram $out/bin/claude \
          --run "source ${preamble}" \
          --run "source ${workspace}" \
          --set-default DISABLE_AUTOUPDATER 1 \
          --set-default KNOWLEDGE_CLAUDE $out/bin/claude-bare \
          --set-default SUDO_ASKPASS ${claude-hooks}/bin/claude-askpass \
          --prefix PATH : ${
            lib.makeBinPath (
              [
                claude-hooks
                claude-statusline
                jj-upload
                knowledge
              ]
              ++ extraPackages
            )
          }

        # Minimal headless claude for narrow tool runs (the curator, skills, route): no
        # MCP servers, slash commands, settings files or CLAUDE.md; the caller picks the tools
        # with `--tools`. Not `--bare`: that drops the subscription login. Wraps the unwrapped
        # claude (no jj workspace). Only CLAUDE_CONFIG_DIR is set (where the login is), not the
        # preamble: that would relink the managed config of whichever claude runs it. Auto-memory
        # is off here because settings.json, where settings.nix turns it off, is not loaded.
        makeWrapper ${claude-code}/bin/claude $out/bin/claude-bare \
          --run 'export CLAUDE_CONFIG_DIR="''${CLAUDE_CONFIG_DIR:-${dataDir}}"' \
          --set-default DISABLE_AUTOUPDATER 1 \
          --set-default CLAUDE_CODE_DISABLE_AUTO_MEMORY 1 \
          --add-flags "--strict-mcp-config --disable-slash-commands --setting-sources="
      '';

      passthru = {
        inherit config;
        inherit (claude-code) version;

        # Overlay-friendly customisation, e.g.
        #   prev.claude.extend { skills.foo = ./foo; settings.permissions.allow = [ "..." ]; }
        # attrsets (settings, agents, skills) deep-merge, extraPackages concatenates,
        # instructions/dataDir are replaced. Results carry `extend` again.
        extend =
          cfg:
          let
            unknown = lib.subtractLists knownKeys (lib.attrNames cfg);
          in
          assert lib.assertMsg (unknown == [ ]) "claude.extend: unknown attributes: ${toString unknown}";
          build {
            instructions = cfg.instructions or instructions;
            dataDir = cfg.dataDir or dataDir;
            settings = deepMerge settings (cfg.settings or { });
            # `null` entries remove an agent/skill, e.g. `skills.commit = null;`
            agents = dropNulls (deepMerge agents (cfg.agents or { }));
            skills = dropNulls (deepMerge skills (cfg.skills or { }));
            extraPackages = lib.unique (extraPackages ++ (cfg.extraPackages or [ ]));
          };
      };

      meta = claude-code.meta // {
        mainProgram = "claude";
      };
    }
  );
in
build {
  instructions = if instructions == null then ./CLAUDE.md else instructions;
  agents = if agents == null then defaultAgents else agents;
  skills = if skills == null then defaultSkills else skills;
  settings = if settings == null then import ./settings.nix else settings;
  inherit extraPackages dataDir;
}
