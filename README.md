# dotfiles

Nix flake monorepo: NixOS hosts, the packages and wrappers they use, and the manifests of the k3s cluster (`dijkstra`) that some of those hosts form. Hosts are impermanent (root is wiped on boot), secrets live in Bitwarden and are turned into sops files at build time, and the cluster is reconciled by Flux.

All commands below are meant to be run from the repo root inside the dev shell.

## Layout

```
flake.nix              inputs, nixosConfigurations, deploy-rs nodes, packages
devenv.nix             dev shell: tools and the scripts described below
secretspec.toml        inventory of every secret: one profile per host, `dijkstra` for the cluster key, `dijkstra-<ns>` for the Secret `secrets` of each namespace
.sops.yaml             age recipients: hosts, user, cluster
secrets-stub/          placeholder for the `secrets` flake input
host/
  <name>/              per-host config, hardware, ssh_host_ed25519_key.pub
  features/
    default.nix        imported by every host: global + sops/impermanence/disko/stylix + modules/host
    global/            always on: networking, nix, openssh, sops
    optional/          opt-in features: k3s, tailscale, docker, libvirt, maintenance, ...
    disko/             disk layouts, parametrised by device (and swap size)
modules/host/          own NixOS modules: persist.nix, secrets.nix, battery-notifier.nix, flux-sops-age.nix, event-router.nix (the router user service, the knowledge curator timers and the shpool user service)
pkgs/                  wrapped packages (package + bundled config) and own tools (pkgs/agent-skills/skills: Agent Skills, shared by claude and agent-skills)
overlays/              exposes pkgs/ (the wrapped packages, shadowing the nixpkgs ones, and the tools), flake inputs as packages, deploy-rs
clusters/dijkstra/     Kubernetes manifests, reconciled by Flux
```

## Hosts

| Host | Arch | Role |
|------|------|------|
| `yenn` | x86_64 | laptop/desktop (Hyprland), LUKS + btrfs. Not a deploy node, rebuilt with `host-switch` |
| `geralt` | x86_64 | server/workstation: docker, libvirt, traefik reverse proxy (mostly inactive) |
| `roach` | x86_64 | k3s agent, NFS server (media), music tooling |
| `regis` | x86_64 | k3s agent, encrypted ZFS (`rpool` + data pool `tank`), NFS |
| `triss` | aarch64 | k3s server (control plane), qemu/virtio guest |
| `alp` | x86_64 | installer ISO, not in `nixosConfigurations` (see `mkiso`) |

`geralt`, `roach`, `regis` and `triss` are deploy-rs nodes (`deploy.nodes` in `flake.nix`): ssh as `maintenance`, interactive sudo.

## Dev shell

`devenv` is loaded by direnv (`.envrc`), or run `devenv shell`. Entering the shell prints the script list and sets `KUBECONFIG=kubeconfig.yaml` (git-ignored). Scripts that touch secrets unlock Bitwarden (`bw`) once and lock it again on exit.

| Script | What it does |
|--------|--------------|
| `host-switch [action]` | `nixos-rebuild <action>` (default `switch`) of the current host, with its secrets injected |
| `host-deploy <node...>` | deploy-rs to the given nodes |
| `install-remote <host> <user@ip>` | first install with nixos-anywhere + disko |
| `secrets-build <outdir> <host...>` | build encrypted sops files from Bitwarden (used by the scripts above) |
| `mkiso` / `writeiso /dev/XXX` | build the `alp` installer ISO / write it to a device |
| `cluster-secrets-build [--check]` | build the cluster's Kubernetes secrets from Bitwarden (see Kubernetes secrets) |
## Installing a new host

1. Add the host: `host/<name>/default.nix`, an entry in `nixosConfigurations` (and `deploy.nodes` if it should be deployed remotely), a disko layout from `host/features/disko/` (or a custom one, see `host/regis/storage.nix`).
2. Generate its ssh host key. The key doubles as the host's sops identity:
   ```sh
   ssh-keygen -t ed25519 -N "" -f host/<name>/ssh_host_ed25519_key
   ssh-to-age < host/<name>/ssh_host_ed25519_key.pub
   ```
   Commit only the `.pub` (the host config reads it). The private half goes to Bitwarden as `HOST_SSH_ED25519_PRIV` (see Secrets). It is not tracked and not git-ignored, so delete the local file after installing.
3. Put the age key in `.sops.yaml` (an anchor under `&hosts` and a `host/<name>/.+$` creation rule) and add a `[profiles.<name>]` to `secretspec.toml` with the host's items in Bitwarden.
4. Boot the target from the ISO: `mkiso`, then `writeiso /dev/sdX`. The ISO joins tailscale with an auth key read from `host/alp/key` (git-ignored, create it in the Tailscale console) and accepts the key of `yenn`'s user over ssh.
5. Put a disk key at `host/<name>/disk.key`. `install-remote` insists on it and passes it to nixos-anywhere as `/tmp/disk.key`; it is not tracked. Only layouts that reference that path use it: `regis`'s `tank` pool (raw key, `dd if=/dev/urandom of=host/regis/disk.key bs=32 count=1`). For such a host, uncomment the zfs lines in `install-remote` (`devenv.nix`) so the key is also copied to `/persist/data/etc/zfs/disk.key`. For other hosts any file will do.
6. Install:
   ```sh
   install-remote <name> root@<ip>
   ```
   It builds the host's secrets, copies the host key to `/persist/data/etc/ssh/ssh_host_ed25519_key` on the new disk and runs nixos-anywhere with the disko script and the toplevel as store paths.

## Updating hosts

```sh
host-switch            # the machine you are on (yenn)
host-switch test       # any nixos-rebuild action
host-deploy geralt roach
```

`host-deploy` builds the secrets for the requested nodes, checks their ssh host keys, answers deploy-rs's sudo prompt with the `maintenance` password from Bitwarden (or asks), and runs `deploy --skip-checks`.

## Secrets

Modules never talk to sops directly. They go through a small contract:

- `modules/host/secrets.nix` defines `config.secrets.<name>` with `owner`, `group`, `mode`, `neededForUsers` and `path` (defaults to `/run/secrets/<name>`). Consumers read only `config.secrets.<name>.path`. `profile` (default: the host's name) names the secretspec profile that holds the secret, for one that lives elsewhere, e.g. `profile = "dijkstra"` for a cluster-wide secret (Bitwarden folder `infra/dijkstra`).
- `host/features/global/sops.nix` is the only place that knows the provider is sops-nix: it maps every `config.secrets` entry to `sops.secrets` and reads `<host>.yaml` from the `secrets` flake input. The age identity is `/persist/data/etc/ssh/ssh_host_ed25519_key`.
- `secretspec.toml` is the inventory. The profile named after the host lists its secrets as uppercased names. Evaluation fails if the secrets used by modules and the profile diverge (missing, unused or badly named; names match `^[a-z0-9_]+$`). Secrets with another `profile` must exist in that profile, which is not required to be used completely by the host.
- The values live in Bitwarden, folder `infra/<profile>` (`bw://?folder=infra/{profile}`).

`secrets-build <outdir> <host...>` exports each profile with `secretspec`, keeps exactly the secrets the config declares, verifies that ssh private keys match the `.pub` files in the repo and that the host's age key (and `FLUX_SOPS_AGE_KEY`, where a host gets it) matches `.sops.yaml`, hashes `*_PASSWORD` entries with `mkpasswd`, and writes `<outdir>/<host>.yaml` encrypted for the host (plaintext never touches disk). Keys marked `composed` in `secretspec.toml` (e.g. `K3S_VPN_AUTH`) are assembled from other keys; the inputs are not written.

The `secrets` flake input is `path:./secrets-stub`, a placeholder with no sops files. Every script that builds a host passes `--override-input secrets path:<outdir>`; to build by hand:

```sh
secrets-build /tmp/s geralt
nixos-rebuild build --flake .#geralt --override-input secrets path:/tmp/s
```

`.sops.yaml` holds the age recipients: a `super` key, one key per host, the user keys, and the cluster key. Host files (`host/<name>/...`) are encrypted for super, the host and the user; `clusters/dijkstra/.+\.enc.+` only for the cluster key.

### Adding a secret

1. Declare and use it in the module that needs it:
   ```nix
   { config, ... }: {
     secrets.myservice_token = { owner = "myservice"; mode = "0400"; };

     services.myservice.tokenFile = config.secrets.myservice_token.path;
   }
   ```
   Use `neededForUsers = true` for anything read while users are created (password hashes); the path then is under `/run/secrets-for-users`.
2. Add it to the profile of every host that imports the module, uppercased:
   ```toml
   [profiles.geralt]
   MYSERVICE_TOKEN = { description = "myservice API token", required = true }
   ```
3. Create the matching item in Bitwarden, folder `infra/geralt`.
4. Check that the config and the inventory agree, then apply:
   ```sh
   nix eval .#nixosConfigurations.geralt.config.secrets --apply builtins.attrNames
   host-deploy geralt
   ```

## Persistence

Root is rolled back on every boot; state survives only in `/persist`, managed by `modules/host/persist.nix` on top of impermanence.

| Option | Meaning |
|--------|---------|
| `persist.enable` | turn the rollback and bind mounts on |
| `persist.deviceService` | systemd device unit to wait for before the rollback, e.g. `dev-nvme0n1p2.device` |
| `persist.rootPath` | device holding the root (btrfs top level, e.g. `/dev/nvme0n1p2` or `/dev/mapper/crypted`) |
| `persist.isCrypted` / `persist.isZfs` | also wait for `cryptsetup.target` / roll back ZFS instead of btrfs |
| `persist.data.{directories,files}` | state worth keeping, under `/persist/data` |
| `persist.generated.{directories,files}` | state that can be regenerated, under `/persist/generated` |
| `persist.users.<user>.{data,generated}` | the same, relative to the user's home |

`generated` always includes `/var/log`, `/var/lib/nixos`, `/var/lib/systemd` and `/etc/machine-id`. On btrfs the old `root` subvolume is moved to `old_roots/<timestamp>` and entries older than 30 days are deleted; on ZFS the rollback is `zfs rollback -r rpool/root@blank` (the snapshot is created by disko in `host/regis/storage.nix`). Home directories are created in both trees on activation.

```nix
persist.generated.directories = [
  { directory = "/var/lib/iwd"; user = "root"; group = "root"; }
];
persist.users.nikodem.data.directories = [ "projects" ];
persist.users.nikodem.generated.files = [ ".ssh/known_hosts" ];
```

## Wrapped packages

`pkgs/<name>/default.nix` wraps an upstream package together with its config, so the config travels with the package instead of living in `$HOME`. The pattern is `symlinkJoin` + `makeWrapper`, with the config copied into `$out`:

```nix
{pkgs, ...}:
pkgs.symlinkJoin {
  name = "rofi";
  paths = [pkgs.rofi];
  buildInputs = [pkgs.makeWrapper];
  postBuild = ''
    wrapProgram $out/bin/rofi \
        --suffix PATH : ${pkgs.lib.strings.makeBinPath [pkgs.uwsm]} \
        --add-flags "-config $out/config/rofi/config.rasi"

    mkdir -p $out/config/rofi
    cp ${./config.rasi} $out/config/rofi/config.rasi
  '';
}
```

Pointing the program at its config is done with a flag (rofi) or an env var (`hyprpaper` sets `XDG_CONFIG_HOME=$out/.config`). To add one: create `pkgs/<name>/` with `default.nix` and the config files, register it in `pkgs/default.nix` (`<name> = pkgs.callPackage ./<name> {<name> = prev.<name>;};`: the wrapper shadows the nixpkgs package of the same name and gets that one as an explicit argument), and use it as `pkgs.<name>` (overlay `additions`, applied last). The same set is exported as flake `packages`, so `nix build .#<name>` works. Wrapped packages can depend on each other (`hyprland` bundles `waybar`, `rofi`, ...).

Packages: claude, dunst, glab, hypridle, hyprland, hyprlock, hyprpaper, jujutsu, kanshi, rofi, signal-desktop, waybar, zellij.

`claude` (`pkgs/claude`, no nixpkgs package of that name; it wraps `claude-code`) is Claude Code with its config bundled: `CLAUDE.md`, `agents/`, `settings.nix` and `permissions.nix` live next to `default.nix`, the skills come from the `agent-skills` package (`pkgs/agent-skills/skills/`). It puts the tools below on its `PATH`, taken from the package set, so overriding one of them propagates. The package is customisable with `.extend { skills.foo = ./foo; settings = ...; }` (see `pkgs/claude/README.md`).

### Tools

Own programs that are not wrappers of an upstream package. They are top-level attributes (`pkgs.<name>`, `nix build .#<name>`):

| Package | What it is |
|---------|------------|
| `knowledge` | knowledge curator CLI (`knowledge submit`, `curate`, ...), keeps the memory repo of claude |
| `jj-upload` | pushes a stack of jj changes as stacked merge/pull requests (GitLab, GitHub) |
| `jw` | jj workspaces at `<repo>.agents/<name>` for the repos under `projectsRoot` (default `~/projects`, set with `.override {projectsRoot = ...;}`; `yenn` uses `/persist/data/home/nikodem/projects`), with a fish function and completions |
| `wallpaper` | the wallpaper image (the store path is the file), used by stylix, hyprpaper and hyprlock |
| `claude-hooks` | hook commands of claude: write/bash guards, architect memory autocommit, event routing, sudo askpass and the permission hook (lets agent-inbox answer permission requests and questions; `CLAUDE_INBOX_WAIT=0` turns that off), the Stop hook (in event-started sessions posts one `claude.waiting` event per turn; the inbox keeps the latest per session, key = session id), queued follow-up prompts of event sessions (`$CLAUDE_EVENT_SESSION_DIR/queue/*.md`; the Stop/UserPromptSubmit hooks hand Claude only their paths), jj workspaces for agents |
| `claude-statusline` | claude's status line |
| `agent-skills` | `pkgs/agent-skills/skills/` in the Agent Skills format, to link into the skills directory of other tools (OpenCode, ...) |
| `event-router` | local HTTP inbox (`event-router emit <type>`) that runs a handler per event type; runs as a systemd user service via the `services.event-router` module. Handlers are the `handlers` argument, extend it with `.override`. Event types listed in the `inbox` argument (prefixes) are also queued in memory: `GET /inbox`, `DELETE /inbox/<id>`; an event with a non-empty string `key` replaces the queued item of the same type and key |
| `agent-inbox` | terminal UI over the router's queue (`claude.*` and `sudo.*` events): list, inspect, dismiss, answer sudo password requests (written to the askpass FIFO, never through the router) and Claude permission prompts and questions (written to the permission hook's FIFO); also `list`, `reply <id>`, `dismiss <id>` |
| `event-handlers.notify` | router handler: desktop notification with an "Open" action (flake: `event-handler-notify`; `notify-critical` is its critical-urgency variant) |
| `event-handlers.knowledge` | router handler for `knowledge.submit` and `knowledge.curate[.weekly]` (flake: `event-handler-knowledge`) |
| `event-handlers.claude` | GitLab event → Claude session: opens a shpool session (`shpool attach <name>`, 1-day TTL; the shpool user service comes from `services.event-router`) with Claude in the project's checkout under `$HOME/<projectsDir>`; `contextTool` (a `gitlab-context prepare` compatible package, default none: plain Claude in the main checkout), `extraPackages`, the `gitlab.*` routes and the watcher are supplied by the work layer (flake: `event-handler-claude`) |

## Infra

`clusters/dijkstra` is a k3s cluster over tailscale:

| Node | Role |
|------|------|
| `triss` | server (traefik and cloud controller disabled) |
| `roach`, `regis` | agents, join `triss` through its tailscale address |

Node config is in `host/features/optional/k3s.nix` plus the per-host `services.k3s` blocks. The token and tailscale join key come from Bitwarden (`K3S_TOKEN`, `K3S_TAILSCALE_AUTH_KEY`). `roach` and `regis` also export NFS (`nfs.nix`), used for app volumes.

The age key Flux decrypts sops manifests with is applied as `flux-system/sops-age` by the k3s server (`services.flux-sops-age`, `modules/host/flux-sops-age.nix`). It is the Bitwarden item `infra/dijkstra/FLUX_SOPS_AGE_KEY` (profile `dijkstra`) and must match `&dijkstra_cluster` in `.sops.yaml`, which the unit checks before applying. Enable it only on the k3s server: moving the server role means moving the flag.

Flux is bootstrapped in `clusters/dijkstra/flux-system` (do not edit `gotk-*.yaml`). It watches `main` of the GitHub repo and applies `./clusters/dijkstra`, so nothing is applied by hand: commit, push, wait. To apply immediately:

```sh
flux reconcile kustomization flux-system --namespace flux-system --with-source
```

Each app is a directory: `arrstack` (sonarr, radarr, lidarr, prowlarr, bazarr, qbittorrent, beets, flaresolverr), `cloudflare` (tunnel), `cron`, `immich`, `jellyfin`, `keda`, `navidrome` (with audiomuse-ai), `ollama`, `omniverse`, `openebs`, `proxy` (traefik, gateway), `shani`, `tailscale` (operator), `velero`. Shared: `storageclass.yaml`.

### Kubernetes secrets

Encrypted with SOPS using the cluster's age key (`.sops.yaml`); Flux decrypts them when applying (`sops-age` secret). Files must match `*.enc.*`. Only `data` and `stringData` are encrypted.

They are generated from Bitwarden by `cluster-secrets-build`, one namespace at a time. The secretspec profile `dijkstra-<ns>` is the Secret `secrets` of namespace `<ns>`, written to `clusters/dijkstra/secrets/<ns>.enc.yaml`. The profile `dijkstra` itself is cluster-level (the age key) and is never rendered.

1. Add the key to `[profiles."dijkstra-<ns>"]` in `secretspec.toml`, e.g. `API_KEY`. Only `[a-z0-9_]` keys are allowed (case aside), and two keys must not differ only by case. As for hosts, a key that only feeds a `composed` key is an input and is not written.
2. Create the Bitwarden item `API_KEY` in the folder `infra/dijkstra-<ns>` (i.e. `infra/dijkstra-<ns>/API_KEY`; secretspec escapes a `/` in a profile name, so namespace profiles use `-`).
3. Run `cluster-secrets-build` and commit `clusters/dijkstra/secrets/`.
4. Reference it in the manifests as secret `secrets`, key `api_key` (keys are lowercased).

The namespace must be created by a `kind: Namespace` manifest in `clusters/dijkstra`. The script decrypts with the cluster key from Bitwarden (`FLUX_SOPS_AGE_KEY`), rewrites only files whose content or age recipient changed, lists each file as `new`, `changed`, `stale` or `unchanged` and never prints values. A key declared in the profile but missing from Bitwarden, or with an empty value, fails; Bitwarden items the profile does not declare are ignored. `cluster-secrets-build --check` writes nothing and exits 1 if any file is out of date; it needs Bitwarden and the cluster private key, so it runs only where those are available. To drop a namespace's secrets, remove its profile: the next run deletes the file.

To rotate the cluster key: put the new age public key in the `&dijkstra_cluster` anchor of `.sops.yaml` and the new private key in the Bitwarden item `FLUX_SOPS_AGE_KEY`, delete `clusters/dijkstra/secrets/*.enc.yaml` (the script refuses to run against files encrypted to the old key), run `cluster-secrets-build`, deploy `triss` (`host-deploy triss`, `flux-sops-age` applies the new key), then commit and push.
