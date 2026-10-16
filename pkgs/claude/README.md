# claude config

`pkgs.claude`: a wrapped `claude` (Claude Code) with the whole config bundled in the Nix store: `CLAUDE.md`, agents, skills, settings, hook scripts and the statusline.

```sh
nix build .#claude                                # build it (from the repo root)
./result/bin/claude                               # try it
```

## Layout

- `pkgs/claude/` - the wrapper (`default.nix`), `CLAUDE.md`, `agents/*.md`, `settings.nix` (rendered to `settings.json`), `permissions.nix` (imported by `settings.nix`) and this README
- `pkgs/agent-skills/skills/` - `*/SKILL.md`, tool-agnostic skills shared by Claude and other tools (`jj`, `atomic-commits`, `commit`, `knowledge`); `claude` takes them from the `agent-skills` package
- `pkgs/knowledge` - the knowledge CLI: `knowledge submit` (queue a note), `curate`, `skills`, `route`, hooks
- `pkgs/jj-upload`, `pkgs/claude-hooks`, `pkgs/claude-statusline`, `pkgs/agent-skills` - the other tools
- `pkgs/default.nix` - registers `claude` and the tools; `overlays/default.nix` exposes them as the `additions` overlay and `flake.nix` exports them as `packages`

`@memoryDir@` and `@dataDir@` in the markdown files and the settings are substituted at build time.

The wrapper takes `knowledge`, `claude-hooks`, `claude-statusline` and `jj-upload` from the package set (`callPackage`) and puts them on its `PATH`, so overriding any of them propagates into `claude`. Its default skills come from `agent-skills` the same way. `pkgs/default.nix` passes `null` for `instructions`, `agents`, `skills` and `settings` so that callPackage never injects a same-named package and the bundled defaults are used.

## Using it in this flake

On a NixOS host `pkgs` already has the `additions` overlay (see `flake.nix`), so just add the package, as `host/yenn/nikodem.nix` does:

```nix
environment.systemPackages = [
  pkgs.claude
  pkgs.jj-upload
];
```

`claude-code` is unfree; the flake's `pkgs` allows it.

The package also ships `claude-bare`, a minimal headless claude for narrow tool runs (used by `knowledge route`, `knowledge skills` and the curator): no MCP servers, slash commands, settings files, `CLAUDE.md` or workspace handling, so a call costs a fraction of the tokens. Pick its tools with `--tools`, e.g. `echo q | claude-bare -p --tools Read,Grep,Glob`. It is not `--bare`, which would drop the subscription login.

## Using it from another flake

Add this repo as an input and apply its `additions` overlay to **your own** nixpkgs:

```nix
{
  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    dotfiles.url = "github:NikodemMarek/dotfiles";
  };

  outputs = { nixpkgs, dotfiles, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        ({ pkgs, ... }: {
          nixpkgs.overlays = [ dotfiles.overlays.additions ];
          nixpkgs.config.allowUnfreePredicate =
            p: builtins.elem (nixpkgs.lib.getName p) [ "claude-code" "claude" ];
          environment.systemPackages = [ pkgs.claude pkgs.jj-upload ];
        })
      ];
    };
  };
}
```

(The same overlay works in `import nixpkgs { overlays = [ ... ]; }` for home-manager or a plain `packages` output.) After changing this repo, `nix flake update dotfiles` in that flake and rebuild.

Use the overlay, not `dotfiles.packages.<system>.claude`: the overlay builds the package with `callPackage` from your package set, so overrides you apply to nixpkgs or to other overlays (`claude-code`, `knowledge`, ...) propagate into it. `packages.*` are already built against this repo's nixpkgs and can't take overrides.

## Settings

`settings.nix` (hooks, statusLine, plugins, `effortLevel`, `theme`, ...; `permissions` live in `permissions.nix`) is rendered to JSON in the store and `$CLAUDE_CONFIG_DIR/settings.json` is a **read-only** symlink to it.

- In-session changes (`/theme`, `/config`, `/plugin`, "don't ask again" permission answers) apply only to the current session and are not persisted.
- To persist a change, edit `settings.nix` (or `permissions.nix`), or use `extend { settings = ...; }` from another layer (see below).
- Project `.claude/settings.json` and `.claude/settings.local.json` work as usual.
- An existing real `settings.json` (or a symlink not managed by Nix) is moved to `$CLAUDE_CONFIG_DIR/backups/settings.json.<YYYYmmdd-HHMMSS>` on first launch (a one-line notice is printed) and replaced by the link.

## Other tools

`agent-skills` holds the shared skills (`pkgs/agent-skills/skills/`) in the Agent Skills `SKILL.md` format: `$out/<name>/SKILL.md`. Link its entries into the skills directory of OpenCode (`~/.config/opencode/skills/`) or Antigravity, e.g. with home-manager:

```nix
xdg.configFile."opencode/skills".source = "${pkgs.agent-skills}";
```

Claude-only frontmatter (`allowed-tools`, and `disable-model-invocation` in `commit`) is ignored by other tools. `pkgs/agent-skills/skills/knowledge` ships with a literal `@memoryDir@` in `agent-skills` (only the `claude` build substitutes it).

## Data dir

On launch the wrapper uses `CLAUDE_CONFIG_DIR` (default: the package's `dataDir`, `$HOME/.local/share/claude`) and sets `CLAUDE_MEMORY_DIR=$CLAUDE_CONFIG_DIR/memory` (always; the agents' prose has that path baked in). The memory dir is `git init`ed if it is not a repo yet.

```
~/.local/share/claude/
  CLAUDE.md          -> /nix/store/...-claude-config/CLAUDE.md
  agents/<name>.md   -> /nix/store/...-claude-config/agents/<name>.md
  skills/<name>      -> /nix/store/...-claude-config/skills/<name>
  settings.json      -> /nix/store/...-claude-config/settings.json (read-only)
  backups/           unmanaged settings.json files moved aside on launch
  memory/            project memory (a git repo, not managed by Nix), shared. The architect owns README.md,
                     INDEX.md, templates/ and projects/<slug>/{overview,decisions,log}.md + notes/; the
                     knowledge curator owns global.md, topic/, lang/, org/ and projects/*/facts.md
                     (`knowledge skills` writes skill-ideas/); SCOPES.toml and private/ are the user's
  ...                sessions, credentials, etc. (claude's own state)
~/.local/state/knowledge/inbox/   the curator's inbox (one file per pending note)
```

Managed entries are symlinks refreshed on every launch; stale ones from older builds are pruned. Real files or foreign symlinks are never overwritten (a warning is printed), except `settings.json`, which is moved to `backups/` first.

`dataDir` is shell-expanded at runtime and must start with `$HOME/` (checked at build time; the markdown prose shows it as `~/...`). Variants (e.g. different `extend` results) sharing one `dataDir` re-link each other's entries on every launch; give a variant its own `dataDir` to run them side by side.

## Migrating from ~/.claude

1. Quit all running claude sessions.
2. `mv ~/.claude ~/.local/share/claude`
3. `mv ~/.claude.json ~/.local/share/claude/.claude.json`
4. Remove the old symlinks to a previous config (e.g. `CLAUDE.md`, `agents/*`, `skills/*`, `settings.json`, hook scripts under `~/.local/share/claude`).
5. A previous memory repo must become the memory dir itself, not a subdirectory of it. Before the first launch: `mv <old memory repo> ~/.local/share/claude/memory`. If the wrapper already created `memory/`: `rm -rf ~/.local/share/claude/memory/.git && mv <old memory repo>/* <old memory repo>/.git ~/.local/share/claude/memory/`.
6. Add `claude` and `jj-upload` to your system packages (see above) and rebuild.
7. Add the jj alias below.

## jj

```toml
[aliases]
upload = ["util", "exec", "--", "jj-upload"]
sync = ["util", "exec", "--", "jj-upload", "sync"]
```

## Extending

`extend` builds a new `claude` with extra config layered on top, so another layer (e.g. a work or private flake) can add its own skills, agents and permission rules without touching this repo:

```nix
let
  claude = pkgs.claude.extend {
    skills.changelog = ./skills/changelog;
    settings.permissions.allow = [ "mcp__grafana__query_loki_stats" ];
    extraPackages = [ pkgs.glab ];
  };
in
{
  environment.systemPackages = [ claude ];
}
```

Or as an overlay, so every consumer of `pkgs.claude` gets the extended one (apply it after `additions`):

```nix
final: prev: {
  claude = prev.claude.extend {
    skills.changelog = ./skills/changelog;
    settings.permissions.allow = [ "mcp__grafana__query_loki_stats" ];
  };
}
```

`extend` accepts `instructions`, `agents`, `skills`, `settings`, `extraPackages` and `dataDir` (other names fail with an error). It deep-merges `settings`, `agents` and `skills` (attrsets recurse, lists such as `permissions.allow` are concatenated and deduplicated), concatenates `extraPackages`, and replaces `instructions` / `dataDir`. A `null` agent/skill removes it: `claude.extend { skills.atomic-commits = null; }`. The result carries `extend` again, so layers chain, and still supports `.override { ... }`, which replaces `instructions` / `agents` / `skills` / `settings` wholesale instead.

## Local/private extensions

Keep private settings, skills or agents out of this repo with a separate flake that takes it as input (see "Using it from another flake" for the overlay and unfree setup):

```nix
{
  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    dotfiles.url = "github:NikodemMarek/dotfiles";
  };

  outputs = { nixpkgs, dotfiles, ... }:
    let
      pkgs = import nixpkgs {
        system = "x86_64-linux";
        overlays = [ dotfiles.overlays.additions ];
        config.allowUnfreePredicate = p: builtins.elem (nixpkgs.lib.getName p) [ "claude-code" "claude" ];
      };
    in
    {
      packages.x86_64-linux.default = pkgs.claude.extend {
        skills.team-review = ./skills/team-review;
        settings.permissions.allow = [ "Bash(make lint:*)" ];
      };
    };
}
```

Or do the same inside your system / home-manager flake: apply `dotfiles.overlays.additions`, then `home.packages = [ (pkgs.claude.extend { ... }) ];`.
