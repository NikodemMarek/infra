# infra

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
modules/host/          own NixOS modules: persist.nix, secrets.nix, battery-notifier.nix, flux-sops-age.nix, agents.nix (the NATS event handler user services, the knowledge curator timer, the `jw-reap` timer that stops idle agent sessions, and the shpool user service)
pkgs/                  wrapped packages (package + bundled config) and own tools (pkgs/agent-skills/skills: Agent Skills, shared by claude and agent-skills)
overlays/              exposes pkgs/ (the wrapped packages, shadowing the nixpkgs ones, and the tools), flake inputs as packages, deploy-rs
clusters/dijkstra/     Kubernetes manifests, reconciled by Flux
tailscale/policy.json  tailnet policy
```

## Hosts

| Host | Arch | Role |
|------|------|------|
| `yenn` | x86_64 | laptop/desktop (Hyprland), LUKS + btrfs. Not a deploy node, rebuilt with `host-switch` |
| `geralt` | x86_64 | headless game streaming (Wolf + Moonlight), docker, libvirt; usually off, woken over LAN |
| `roach` | x86_64 | k3s agent |
| `regis` | x86_64 | k3s agent, encrypted ZFS (`rpool` + data pool `tank`), NFS server, Garage (S3 for Velero) |
| `triss` | aarch64 | k3s server (control plane), qemu/virtio guest |
| `alp` | x86_64 | installer ISO, not in `nixosConfigurations` (see `mkiso`) |

`geralt`, `roach`, `regis` and `triss` are deploy-rs nodes (`deploy.nodes` in `flake.nix`): ssh as `maintenance`, interactive sudo.

Cluster access: the dev shell sets `KUBECONFIG` to `kubeconfig.yaml` in the `default` jj workspace root, so `kubectl` works from the workstation. On `triss` itself `k3s.yaml` is root-only (`sudo kubectl`).

## Game streaming (geralt)

`host/features/optional/wolf.nix` runs [Wolf](https://github.com/games-on-whales/wolf) (Moonlight server) as a docker container, pinned by image digest.

- Wake: `wake-geralt` on `yenn` (runs `ssh maintenance@regis wakeonlan -i 192.168.18.255 b4:2e:99:a4:86:1c`). `regis` must be cabled to geralt's LAN and wake-on-PCIe enabled in the BIOS. `.link` changes apply only when the NIC appears, so reboot once after deploying them.
- Sleep (`host/geralt/idle-suspend.nix`): suspends after 30 min without a Moonlight stream or SSH login, hibernates after 2 h asleep. Pause with `sudo systemd-run --unit=stay-awake systemd-inhibit --what=sleep sleep 4h` (cancel: `sudo systemctl stop stay-awake`); see why it did or didn't with `journalctl -u idle-suspend`. Reboot after kernel updates: a hibernate image can't resume into a new kernel.
- Pair: add `geralt` in Moonlight (over tailscale); it shows a PIN. `docker logs -f wolf` on geralt prints `Insert pin at http://<ip>:47989/pin/#<secret>`: open that with geralt's tailscale address and enter the PIN.
- State lives in `/etc/wolf` (persisted under `/persist/data`): `cfg/config.toml` (Wolf-owned, holds the paired clients, edited by hand), certs, per-client app homes with game libraries. Typical hand edits: add Heroic (image `ghcr.io/games-on-whales/heroic-games-launcher:edge`) and set the apps' `DeviceCgroupRules` hidraw major `244` to geralt's (`grep hidraw /proc/devices`).

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
   Commit only the `.pub` (the host config reads it). The private half goes to Bitwarden as `HOST_SSH_ED25519_PRIV` (see Secrets). The private key is git-ignored (`host/*/ssh_host_ed25519_key`), but delete the local file after installing.
3. Put the age key in `.sops.yaml` (an anchor under `&hosts` and a `host/<name>/.+$` creation rule) and add a `[profiles.<name>]` to `secretspec.toml` with the host's items in Bitwarden.
4. Boot the target from the ISO: `mkiso`, then `writeiso /dev/sdX`. The ISO joins tailscale with an auth key read from `host/alp/key` (git-ignored, create it in the Tailscale console) and accepts the key of `yenn`'s user over ssh.
5. For a host with a ZFS pool, put a disk key at `host/<name>/disk.key` (git-ignored; `regis`'s `tank` pool uses a raw key, `dd if=/dev/urandom of=host/regis/disk.key bs=32 count=1`). `install-remote` requires it only if the host's disko config has a zpool: it passes it to nixos-anywhere as `/tmp/disk.key` and also copies it to `/persist/data/etc/zfs/disk.key`. Other hosts need no disk key.
6. Install:
   ```sh
   install-remote <name> root@<ip>
   ```
   It builds the host's secrets, copies the host key to `/persist/data/etc/ssh/ssh_host_ed25519_key` on the new disk and runs nixos-anywhere with the disko script and the toplevel as store paths.
7. For a k3s node, join tailscale by hand. k3s keeps restarting until the node is logged in, and each start runs `tailscale up` with its placeholder `--vpn-auth` key (ignored once logged in), so stop it while joining:
   ```sh
   sudo systemctl stop k3s
   sudo tailscale up --auth-key=<key>
   sudo systemctl start k3s
   ```
   Generate the key in the Tailscale console (Settings > Keys): single use, 1-day expiry, tag `tag:server`. Tagged nodes don't expire, and the policy auto-approves the pod CIDR route k3s advertises. Add the node's tailscale IP to `ips` in `host/features/optional/k3s.nix`.

## Updating hosts

```sh
host-switch            # the machine you are on (yenn)
host-switch test       # any nixos-rebuild action
host-deploy geralt roach
```

`host-deploy` builds the secrets for the requested nodes, checks their ssh host keys, answers deploy-rs's sudo prompt with the `maintenance` password from Bitwarden (or asks), and runs `deploy --skip-checks`.

## Secrets

Modules never talk to sops directly. They go through a small contract:

- `modules/host/secrets.nix` defines `config.secrets.<name>` with `owner`, `group`, `mode`, `neededForUsers` and `path` (defaults to `/run/secrets/<name>`). Consumers read only `config.secrets.<name>.path`. `profile` (default: the host's name) names the secretspec profile that holds the secret, for one that lives elsewhere, e.g. `profile = "dijkstra"` for a cluster-wide secret (Bitwarden items titled `infra/dijkstra/<KEY>`).
- `host/features/global/sops.nix` is the only place that knows the provider is sops-nix: it maps every `config.secrets` entry to `sops.secrets` and reads `<host>.yaml` from the `secrets` flake input. The age identity is `/persist/data/etc/ssh/ssh_host_ed25519_key`.
- `secretspec.toml` is the inventory. The profile named after the host lists its secrets as uppercased names. Evaluation fails if the secrets used by modules and the profile diverge (missing, unused or badly named; names match `^[a-z0-9_]+$`). Secrets with another `profile` must exist in that profile, which is not required to be used completely by the host.
- The values live in Bitwarden, one item per secret titled `infra/<profile>/<KEY>` (`bw://?folder=infra/{profile}` is a prefix of the item title, there are no Bitwarden folders).

`secrets-build <outdir> <host...>` exports each profile with `secretspec`, keeps exactly the secrets the config declares, verifies that ssh private keys match the `.pub` files in the repo and that the host's age key (and `FLUX_SOPS_AGE_KEY`, where a host gets it) matches `.sops.yaml`, hashes `USERS_*_PASSWORD` entries (login passwords) with `mkpasswd`, and writes `<outdir>/<host>.yaml` encrypted for the host (plaintext never touches disk). Keys marked `composed` in `secretspec.toml` (e.g. `CLOUD` in `dijkstra-velero`) are assembled from other keys; the inputs are not written.

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
3. Create the matching item in Bitwarden, titled `infra/geralt/MYSERVICE_TOKEN`.
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
{
  pkgs,
  rofi,
  ...
}:
pkgs.symlinkJoin {
  name = "rofi";
  paths = [rofi];
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

Pointing the program at its config is done with a flag (rofi) or an env var (`hyprpaper` sets `XDG_CONFIG_HOME=$out/.config`). To add one: create `pkgs/<name>/` with `default.nix` and the config files, register it in `pkgs/default.nix` (`<name> = pkgs.callPackage ./<name> {<name> = prev.<name>;};`: the wrapper shadows the nixpkgs package of the same name and gets that one as an explicit argument), and use it as `pkgs.<name>` (overlay `additions`, applied last). Every derivation of that set is exported as a flake package (`nix build .#<name>`); the event handlers are `nix build .#event-handlers.<name>`. Wrapped packages can depend on each other (`hyprland` bundles `waybar`, `rofi`, ...).

Packages: bitwarden-cli, claude, dunst, gh, glab, heroic, hypridle, hyprland, hyprlock, hyprpaper, jujutsu, kanshi, moonlight-qt, nats-server, obsidian, prismlauncher, rofi, waybar, zellij, zen-browser.

Apps that keep their state in `$HOME` (`bitwarden-cli`, `gh`, `heroic`, `moonlight-qt`, `obsidian`, `prismlauncher`, `zen-browser`) take an optional `dataDir`: with the default `null` the package is the upstream one unchanged, with `.override {dataDir = "/persist/generated/home/<user>/<app>";}` it is wrapped by `jail`, which points the app's config/data env var (`XDG_CONFIG_HOME`, `GH_CONFIG_DIR`, ...) at that directory so the state lands in a persisted path.

`claude` (`pkgs/claude`, no nixpkgs package of that name; it wraps `claude-code`) is Claude Code with its config bundled: `CLAUDE.md`, `agents/`, `settings.nix` and `permissions.nix` live next to `default.nix`, the skills come from the `agent-skills` package (`pkgs/agent-skills/skills/`). It puts the tools below on its `PATH`, taken from the package set, so overriding one of them propagates. The package is customisable with `.extend { skills.foo = ./foo; settings = ...; }` (see `pkgs/claude/README.md`).

### Tools

Own programs that are not wrappers of an upstream package. They are top-level attributes (`pkgs.<name>`, `nix build .#<name>`; `jail` is a function and not built):

| Package | What it is |
|---------|------------|
| `knowledge` | knowledge CLI of claude's memory repo: `knowledge submit` queues a note, `curate` (the user timer, every 6 h) merges the notes into the scope files, `skills` drafts skill ideas on demand, `hook` is the session-start/subagent-stop hook; `knowledge route` picks the repo a task belongs in |
| `jj-upload` | pushes a stack of jj changes as stacked merge/pull requests (GitLab, GitHub) |
| `jw` | jj workspaces at `<repo>.agents/<name>` for the repos under `projectsRoot` (default `~/projects`, set with `.override {projectsRoot = ...;}`; `yenn` uses `/persist/data/home/nikodem/projects`), with a fish function and completions; `jw new -r REV` starts on a revision or bookmark; `jw agent [-d] [-r REV] [repo] [--] [prompt]` always runs Claude in a new `s-MMDD-HHMMSS` workspace (on `trunk()` or REV) in a shpool session of the same name (`AGENT_SESSION=<name>`, with a session record of kind `task`), and fails for a non-jj repo; outside a repo and with a terminal, `jw agent "<task>"` picks the repo the task fits (`knowledge route`; the picker if unsure), without one (e.g. from an event handler) it fails if no repo fits; event sessions (`KNOWLEDGE_ORIGIN=event:*`) cannot start agents; `jw pick` picks a repo with fzf (the current one first) and prints its name (exit 130 if cancelled); `jw attach [name]` attaches to a Claude shpool session (picker if no name, a unique prefix is enough); `jw msg [--from SENDER] NAME [--] [text]` writes the text (quoted, under a `Message from SENDER:` header) to the inbox socket of the running Claude session NAME (its name or its workspace/shpool name; text from stdin if none; refused in GitLab event sessions): it arrives between the tool calls of a busy session or starts a turn in an idle one, and fails if no such session runs (one just started is waited for, up to 10 s); `jw session get` / `jw session set` (both take `[-s NAME]`) read or updates the session's record (`$XDG_STATE_HOME/agent-sessions/NAME.json`: kind, title, description, url, workspace, mrs; NAME defaults to `$AGENT_SESSION`; `set` is refused in GitLab event sessions) |
| `wallpaper` | the wallpaper image (the store path is the file), used by stylix, hyprpaper and hyprlock |
| `claude-hooks` | hook commands of claude: write/bash guards, architect memory autocommit, event routing, sudo askpass and the permission hook (lets agent-inbox answer permission requests and questions; `CLAUDE_INBOX_WAIT=0` turns that off; only for sessions with `AGENT_SESSION`, which routed events carry as `session`), the Stop hook (in sessions with `AGENT_SESSION` posts one `claude.waiting` event per turn; the inbox keeps the latest per session, key = session id), queued follow-up prompts of event sessions (`$CLAUDE_EVENT_SESSION_DIR/queue/*.md`; the Stop/UserPromptSubmit hooks hand Claude only their paths), jj workspaces for agents |
| `claude-statusline` | claude's status line |
| `agent-skills` | `pkgs/agent-skills/skills/` in the Agent Skills format, to link into the skills directory of other tools (OpenCode, ...) |
| `event-bus` | events over plain NATS (local server, `$NATS_URL`): `event-pub TYPE` publishes a JSON object from stdin on `events.TYPE`; `event-subscribe HANDLER SUBJECT...` runs HANDLER per event (JSON on stdin, type in `$ER_TYPE`). |
| `event-handlers` | the NATS event handlers as one systemd user service `event-handlers` (`services.agents` links the unit shipped in the package): the plain file `pkgs/event-handlers/handlers` has one `<subject> <handler>` line per subscription (`#` comments; a handler is a command of the package, e.g. `event-handler-notify-normal`, or an absolute path) and the service runs one `event-subscribe` per handler, restarting them all if one exits. Keep subjects under `events.`: the leaf link only denies `events.>`. Replace the file with `.override {handlers = ...;}`, or append: `concatText "handlers" [prev.event-handlers.handlers (writeText "extra" "events.gitlab.> ${...}\n")]`. A malformed file or an unknown handler fails the build. The handlers below are its attributes (`pkgs.event-handlers.<name>`) |
| `agent-inbox` | terminal UI over the inbox directory `$XDG_RUNTIME_DIR/agent-inbox/` (`claude.*` and `sudo.*` events; only sessions started by `jw agent` or the GitLab event handler, i.e. with `AGENT_SESSION`, hand-started ones get the local dialog only, sudo prompts always route): rows show the title of the session record (`$XDG_STATE_HOME/agent-sessions/<name>.json`, read again every second), the detail pane its links and description, `o` opens its `url` or first MR (https only), Tab (Shift-Tab back) cycles the filter tabs (package arg `tabs`, a list of `{name; kinds ? null;}`) and the Sessions tab (live sessions; `a` attach, `x` kill, Enter their detail): list, inspect, dismiss, answer sudo password requests (written to the askpass FIFO, never published) and Claude permission prompts and questions (written to the permission hook's FIFO), attach to an item's shpool session in a zellij tab (`a`); `N` publishes a `task.new` with `event-pub` to start an agent (an empty task: `jw pick` a repo first, no attach; the session shows up in the Sessions tab, and a task that could not be published stays in the popup); `task.failed` items show in every tab, `p` picks a repo and sends the task again; also `list`, `reply <id>`, `dismiss <id>` and `sessions` (the live sessions as TSV: pid, status, age, shpool session, place, session id) |
| `tm` | CLI for the markdown task list `$TM_DIR/tasks.md` (`TM_DIR` is required): one rolling backlog where the checkbox is the state (`[ ]` todo, `[/]` doing, `[x]` done). `add`, `start`, `stop`, `done`, `list`, `lint`; edits one line at a time and never moves lines, so untouched lines stay byte-identical. `tm serve` exposes the same commands over HTTP: an MCP server at `https://tasks.nkmrk.com/mcp` and `POST https://tasks.nkmrk.com/api/add` (body = the task text), for full WireGuard peers only |
| `tm-image` | docker image of `tm serve` (port 8080) for the cluster, built into `triss` (see `tm` on the cluster) |
| `dictate` | records from the default microphone until Enter, transcribes with the local whisper-server (`host/features/optional/dictate.nix`, `DICTATE_WHISPER_URL`) and prints the text on stdout |
| `agent-task` | quick capture of a task: a rofi prompt, optionally after a screenshot (`--shot`) and with the clipboard image attached on Ctrl+Enter; publishes `task.new` with `event-pub` |
| `jail` | a function, not a package (no `nix build`): `jail <package> {ENV_VAR = "/abs/path";}` wraps every binary so it runs with those env vars set to fixed directories (created 0700), see the `dataDir` apps above |
| `event-handlers.notify` | event handler: desktop notification with an "Open" action (flake: `event-handlers.notify`; `notify-critical` is its critical-urgency variant) |
| `event-handlers.task` | event handler for `task.new {prompt?, repo?, images?}` (posted by `agent-task` and agent-inbox): starts an agent with `jw agent -d` and posts `task.started`, or `task.failed` with what is needed to retry; only images under `$XDG_RUNTIME_DIR/agent-task/` are passed on (flake: `event-handlers.task`) |
| `event-handlers.inbox` | event handler that queues the event as `$XDG_RUNTIME_DIR/agent-inbox/<id>.json` (`{id, received, event}`; an event with a non-empty string `key` replaces the item of the same type and key, at most 200 are kept; flake: `event-handlers.inbox`) |
| `event-handlers.remote-notify` | event handler for `remote.notify.>` from the cluster (untrusted; the leaf link denies `events.>` and the system subjects, so local events never cross). Producer contract: publish to `remote.notify.<anything>` a one-line JSON object `{"type":"remote.notify","title":…,"body"?:…,"project"?:…}` (`type` must be present and valid, its value is replaced; `title` a non-empty string, cut to 200 chars; `body` a string, cut to 1000; `project` matching `[A-Za-z0-9._-]{1,64}`, else `remote`). Every other field is dropped; the result goes to `event-handlers.notify` (flake: `event-handlers.remote-notify`) |
| `event-handlers.claude` | GitLab event → Claude session: opens a shpool session (`jw attach <name>`, 1-day TTL; the shpool user service comes from `services.agents`) with Claude in the project's checkout under `$HOME/<projectsDir>`; if the context tool returns the optional `session` field, it continues the MR's one conversation (resume), and a live session only gets a "queued" notification and picks the update up through its hooks; `contextTool` (a `gitlab-context prepare` compatible package, default none: plain Claude in the main checkout), `extraPackages`, the `gitlab.*` routes and the watcher are supplied by the work layer (flake: `event-handlers.claude`) |

## Infra

`clusters/dijkstra` is a k3s cluster over tailscale:

| Node | Role |
|------|------|
| `triss` | server (traefik and cloud controller disabled) |
| `roach`, `regis` | agents, join `triss` through its tailscale address |

Node config is in `host/features/optional/k3s.nix` plus the per-host `services.k3s` blocks. The token comes from Bitwarden (`K3S_TOKEN`). `regis` also exports NFS (`nfs.nix`), used for app volumes.

The tailnet policy is `tailscale/policy.json`. Change it in the repo, then paste it into the Tailscale console (Access controls > JSON editor) and save; don't edit it in the console.

The age key Flux decrypts sops manifests with is applied as `flux-system/sops-age` by the k3s server (`services.flux-sops-age`, `modules/host/flux-sops-age.nix`). It is the Bitwarden item `infra/dijkstra/FLUX_SOPS_AGE_KEY` (profile `dijkstra`) and must match `&dijkstra_cluster` in `.sops.yaml`, which the unit checks before applying. Enable it only on the k3s server: moving the server role means moving the flag.

Flux is bootstrapped in `clusters/dijkstra/flux-system` (do not edit `gotk-*.yaml`). It watches `main` of the GitHub repo and applies `./clusters/dijkstra`, so nothing is applied by hand: commit, push, wait. To apply immediately:

```sh
flux reconcile kustomization flux-system --namespace flux-system --with-source
```

Each app is a directory: `arrstack` (sonarr, radarr, lidarr, prowlarr, bazarr, qbittorrent (egress via gluetun/Mullvad), beets, flaresolverr), `cert-manager` (Let's Encrypt wildcard for internal HTTPS), `cloudflare` (tunnel; only the hostnames listed in `cloudflared.yaml` are public), `cnpg` (CloudNativePG operator), `cron`, `dns` (split-DNS resolver for nkmrk.com for WireGuard peers, at `10.43.0.53`), `immich` (`photos.nkmrk.com`), `intel-gpu-plugin` (exposes regis's iGPU as `gpu.intel.com/i915`), `jellyfin` (`movies.nkmrk.com`, with seerr at `seerr.nkmrk.com`), `keda`, `matrix` (Synapse, `matrix.nkmrk.com`), `navidrome` (`music.nkmrk.com`, with audiomuse-ai), `ollama`, `omniverse` (git-sync, webdav, radicale, tasks), `openebs`, `proxy` (traefik, gateway), `shani` (SSH at `shani.nkmrk.com`, full WireGuard peers), `velero`, `webdav` (books, `books.nkmrk.com`, public through cloudflared), `wireguard` (WireGuard server on `triss`, egress via Mullvad). Shared: `storageclass.yaml`.

The `shani` image is built and imported onto `triss` by hand (the deployment uses `imagePullPolicy: Never`, `clusters/dijkstra/shani/deployment.yaml`), so a new image needs its tag set in the manifest and importing on `triss` before the pod can start.

Services on the gateway's `internal` listener (full WireGuard peers only, HTTPS; `dns` resolves these names to `traefik-internal`): `radicale`, `sonarr`, `radarr`, `lidarr`, `qbittorrent`, `bazarr`, `prowlarr`, `ollama`, `audiomuse-ai`, `tasks` (tm MCP), `matrix` (Synapse) and `omniverse` (WebDAV), each at `<name>.nkmrk.com`, and immich at `photos.nkmrk.com`. `bazarr` and `prowlarr` go through the KEDA interceptor (`keda/httproute.yaml`), so their host is also listed in their `HTTPScaledObject`. A new internal name needs an HTTPRoute on `sectionName: internal` and an entry in the `dns` Corefile.

AudioMuse-AI (`navidrome/audiomuse-ai`) keeps its settings (Navidrome login, auth/API token, AI provider) in its own database, set in its web UI at `https://audiomuse-ai.nkmrk.com/`. Navidrome's `audiomuseai` plugin is configured in Navidrome's UI with that token.

Radicale (`omniverse/radicale`) serves CalDAV at `https://radicale.nkmrk.com/` (full WireGuard peers only, through the gateway's `internal` listener), user `nikodem`. Its data is the omniverse `dav/` directory, versioned by git-sync except what the `omniverse-gitignore` ConfigMap excludes (the `sub-*` subscription calendars and Radicale runtime files). `dav/` is not served over the omniverse WebDAV because Radicale is its only writer: use CalDAV (DAVx⁵ on Android).

Calendar subscriptions are listed in the secret `CALENDAR_SUBSCRIPTIONS` (profile `dijkstra-omniverse`, rebuild with `cluster-secrets-build`), one `<name> <url> [<display name>]` per line: `<name>` is lowercase letters, digits and dashes, starting with a letter or digit; `<url>` is `http(s)://` or `webcal://`; the display name (may contain spaces) defaults to `<name>`. All other lines are ignored. The CronJob `calendar-subscriptions` refreshes them daily at 04:00 Europe/Warsaw into read-only `sub-<name>` calendars. Run it now with:

```bash
kubectl -n omniverse create job --from=cronjob/calendar-subscriptions refresh-now
```

`tm` on the cluster: the image is built into triss (`services.k3s.images`) and k3s imports it. When `nix eval --raw .#nixosConfigurations.triss.pkgs.tm-image.imageTag` differs from the tag in `clusters/dijkstra/omniverse/tasks/deployment.yaml` (also after flake updates), put the new tag there, `host-deploy triss`, then push. If the pod is stuck in `ErrImageNeverPull`, run `sudo k3s ctr -n k8s.io images import /var/lib/rancher/k3s/agent/images/nixos/*-docker-image-tm.tar` on triss, then `kubectl -n omniverse rollout restart deploy/tasks`.

To remove a subscription, delete its line, then delete the calendar (read-only over CalDAV):

```bash
kubectl -n omniverse exec deploy/radicale -- rm -r /data/collection-root/nikodem/sub-<name>
```

Matrix (`matrix/synapse`) is a private, non-federated Synapse (`server_name` `nkmrk.com`, permanent) at `https://matrix.nkmrk.com/` (full WireGuard peers only, through the gateway's `internal` listener, client API only: `/_synapse` admin is not routed through the gateway). Log in by typing `matrix.nkmrk.com` as the homeserver. Registration is closed; create users with the shared secret (`REGISTRATION_SHARED_SECRET`, profile `dijkstra-matrix`):

```bash
kubectl -n matrix exec -it deploy/synapse -- register_new_matrix_user -c /config/homeserver.yaml http://localhost:8008
```

Its database is the CloudNativePG cluster `matrix-postgres`, backed up and restored like the Immich database (a `pg_dump` pre-hook, `synapse.dump`; database and owner `synapse`); the signing key and media are on the PVC `synapse-data`.

Bridges are mautrix appservices in `matrix/<network>`, one per network (`whatsapp`, `meta`, pinned image, SQLite on its own PVC). For WhatsApp, DM `@whatsappbot:nkmrk.com` and send `login qr`; for Messenger, DM `@metabot:nkmrk.com`, send `login` and pick Messenger. Bump the bridge images about monthly: a "client outdated" notice in the bridge room is the cue.

### WireGuard peers

Peers have one of two tiers, set by their address. Limited peers (`10.13.13.2-127`) reach only `*.nkmrk.com`, the cluster DNS server (`10.43.0.53`) and the internet (via Mullvad), never the cluster network. Full peers (`10.13.13.128-254`) also reach the cluster's pod and service networks (`10.42.0.0/15`). The pod's firewall enforces the tiers. Peers' internet leaves through the pod's own Mullvad WireGuard tunnel (relay `de-fra-wg-001`, pinned in `wireguard/deployment.yaml`); without the tunnel peers have no internet. To change the relay, take its IPv4 and public key from Mullvad's relay list (`api.mullvad.net/public/relays/wireguard/v2`).

1. Generate a key pair: `umask 077; wg genkey > p.key; wg pubkey < p.key`.
2. Add a `[Peer]` (name comment, `PublicKey`, next free `10.13.13.N/32` in its tier) to `clusters/dijkstra/wireguard/wg0.conf`, commit and push; the pod restarts with it.
3. Write the client config `client.conf`:
   ```ini
   [Interface]
   PrivateKey = <p.key>
   Address = 10.13.13.N/32
   DNS = 10.43.0.53
   MTU = 1200

   [Peer]
   PublicKey = <server public key, see wg0.conf>
   Endpoint = vpn.nkmrk.com:51820
   AllowedIPs = 0.0.0.0/0, ::/0
   PersistentKeepalive = 25
   ```
4. `qrencode -t ansiutf8 < client.conf`, scan it, then delete `p.key` and `client.conf`.

`yenn` is a NixOS peer (`host/yenn/wireguard.nix`); its private key is `WIREGUARD_PRIVATE_KEY` in profile `yenn`. All its internet except the tailnet and directly attached networks goes through the tunnel, and its DNS always goes to the cluster `dns` resolver at `10.43.0.53`, which also resolves cluster services as `<service>.<namespace>.svc.cluster.local`. With the tunnel down there is no internet: `sudo networkctl down wg0` (`up` to restore), or click the waybar WireGuard button. Behind a captive portal take the tunnel down and run `sudo resolvectl default-route wlan0 yes`; after logging in, `sudo resolvectl default-route wlan0 no` and bring the tunnel back up.

### Backups

Velero backs up the cluster daily at 18:00 Europe/Warsaw and keeps each backup for 30 days. The target is Garage on `regis` (`host/regis/garage.nix`): a single-node S3 store on the `tank/garage` dataset, bucket `velero` at `http://100.100.215.91:3900`, reachable only over `tailscale0` and `cni0`. Velero and Garage share one key pair, from Bitwarden `infra/dijkstra-velero/ACCESS_KEY_ID` and `SECRET_ACCESS_KEY`; Garage creates the layout, the bucket and the key on startup (`--single-node --default-bucket`).

- Backed up: every namespace except `kube-system`, `velero` and `flux-system`, without Secrets (they come from Bitwarden), and the pod volumes through kopia, except `local-storage` (the media on tank), `nfs`, emptyDir and PVCs labelled `backup: skip` (downloads, caches).
- The beets library (`library.db`) lives on its own PVC `beets-data`, mounted only by the always-on beets pod, so Velero backs it up with the rest. To restore it, `velero restore` that PVC; delete a leftover `library.db-journal` next to it first, or sqlite rolls it back onto the restored file.
- Nothing alerts on a failed backup: check `velero backup get` now and then.

#### Restore

App volumes are provisioned dynamically (openebs-zfs on `regis`, openebs-hostpath on `triss`, both Retain), so a plain Velero restore works. To restore one app:

```sh
flux suspend kustomization flux-system -n flux-system
kubectl -n <namespace> delete deploy --all
kubectl -n <namespace> get pvc -L backup
kubectl -n <namespace> delete pvc <the openebs-* PVCs without backup=skip>
velero restore create --from-backup <backup> --include-namespaces <namespace> --wait
velero restore describe <restore> --details
flux resume kustomization flux-system -n flux-system
```

- Delete only the backed-up `openebs-*` PVCs. Keep the `backup: skip` ones and the static `local-storage`/`nfs` ones: Velero restores their PV as is, so a deleted one stays Pending.
- A PVC shared by several Deployments (`arrstack-config-pvc`, `omniverse-pvc`) is restored through only one of their pods; the others start on the empty volume and write to it (fresh SQLite databases, a git clone) before the restore finishes. Restore those volumes with kopia (below) instead.
- Don't delete the namespace: the static media PVs would go Released with a stale `claimRef` and the restored PVCs wouldn't bind.
- Arrstack apps are scaled by KEDA: delete their HTTPScaledObjects first (`kubectl -n arrstack delete httpscaledobject --all`). Flux is suspended, so it doesn't recreate them until you resume.
- The replaced volumes stay as Released PVs (Retain). Once the app works, clean up with `kubectl delete pv <pv>`, then `kubectl -n openebs delete zfsvolume <pv>` (destroys the dataset) for openebs-zfs, or remove `/var/openebs/local/<pv>` on `triss`.

##### Restore a volume with kopia

For shared volumes, or when a Velero restore fails, restore the data with kopia into the existing volume:

1. Suspend Flux (it resets `replicas`), scale the app to 0, and delete its HTTPScaledObject if KEDA scales it:
   ```sh
   flux suspend kustomization flux-system -n flux-system
   kubectl -n <namespace> scale deploy/<app> --replicas=0
   ```
2. On the node holding the volume, connect read-only to the namespace's repository. The password is Velero's static default (secret `velero/velero-repo-credentials`), so this works without the cluster:
   ```sh
   sudo nix run nixpkgs#kopia -- repository connect s3 --endpoint 100.100.215.91:3900 --disable-tls --bucket velero --prefix kopia/<namespace>/ --region garage --access-key <GK...> --secret-access-key <secret> --password static-passw0rd --readonly
   ```
3. Find the snapshot ID (without the cluster: `kopia snapshot list --all`):
   ```sh
   kubectl -n velero get podvolumebackups -l velero.io/backup-name=<backup> -o custom-columns=VOL:.spec.volume,POD:.spec.pod.name,SNAP:.status.snapshotID
   ```
4. Find the volume's directory `<path>` and a place `<aside>` for its current contents outside it (`<pv>` is the PVC's `spec.volumeName`):
   - `triss`: `<path>` is `/var/openebs/local/<pv>`, `<aside>` is `/var/openebs/local/<pv>.pre-restore`.
   - `regis`: mount the dataset (it is not mounted while the app is down); `<path>` is `/mnt/restore`, `<aside>` is `/mnt/openebs/pre-restore-<pv>`, outside the dataset so its quota doesn't count both copies:
     ```sh
     sudo mkdir -p /mnt/restore && sudo mount -t zfs rpool/openebs/<pv> /mnt/restore
     ```
5. Restore into an empty directory, because leftover SQLite `-wal`/`-shm` files corrupt a restored database:
   ```sh
   sudo mkdir <aside>
   sudo find <path> -mindepth 1 -maxdepth 1 -exec mv -t <aside> {} +
   sudo nix run nixpkgs#kopia -- snapshot restore <snapshotID> <path>
   sudo nix run nixpkgs#kopia -- repository disconnect
   ```
   On `regis`, `sudo umount /mnt/restore` afterwards.
6. `flux resume kustomization flux-system -n flux-system`, check the app, then delete `<aside>`.

#### Immich database

The Immich database is the CloudNativePG cluster `immich-postgres` (`clusters/dijkstra/immich/postgres.yaml`). Its PVC is copied by Velero like any other, but live PGDATA is not a consistent backup: the daily 18:00 backup first runs a pre-hook that writes `immich.dump` (`pg_dump -Fc`) to the PVC root, then copies the PVC. Restore only from that dump.

##### Restore the database

Run everything from the workstation (dev shell). `<pod>` is the primary, `kubectl -n immich get cluster immich-postgres -o jsonpath='{.status.currentPrimary}'` (usually `immich-postgres-1`).

1. Make sure the Cluster from git is healthy (`kubectl -n immich get cluster immich-postgres`); if it is missing, let Flux recreate it and wait. Any hand-written bootstrap must set `database: immich` and `owner: immich`, otherwise CNPG creates an empty `app` database.
2. Suspend Flux and stop Immich:
   ```sh
   flux suspend kustomization flux-system -n flux-system
   kubectl -n immich scale deploy/immich --replicas=0
   ```
3. Connect read-only to the `immich` repository (step 2 of "Restore a volume with kopia", without `sudo`) and find the snapshot of volume `pgdata` of `<pod>` (step 3 there).
4. Restore it to a scratch directory and copy the dump into the pod:
   ```sh
   nix run nixpkgs#kopia -- snapshot restore <snapshotID> ./immich-pgdata
   nix run nixpkgs#kopia -- repository disconnect
   kubectl -n immich cp -c postgres ./immich-pgdata/immich.dump <pod>:/var/lib/postgresql/data/restore.dump
   ```
5. In the pod (`kubectl -n immich exec -it <pod> -c postgres -- bash`):
   ```sh
   dropdb --force immich && createdb -O immich immich
   pg_restore -d immich --exit-on-error --single-transaction /var/lib/postgresql/data/restore.dump
   pg_restore -C -f - /var/lib/postgresql/data/restore.dump | grep -E '^ALTER DATABASE .* SET ' | psql -d postgres
   psql -d immich -c ANALYZE
   rm /var/lib/postgresql/data/restore.dump
   ```
   Without `--create`, `pg_restore` drops database-level settings such as `vchordrq.probes`; the second `pg_restore` replays them.
6. `flux resume kustomization flux-system -n flux-system`, check Immich, then delete `./immich-pgdata`.

Don't `velero restore` the `immich` namespace for the database: it would recreate operator-owned objects.

The same pre-hook and restore procedure applies to `audiomuse-postgres` (database `audiomuse`, namespace `navidrome`, `clusters/dijkstra/navidrome/audiomuse-ai/postgres.yaml`): substitute that cluster, namespace, database and owner (`audiomuse`), the dump name (`audiomuse.dump`) and the AudioMuse-AI deployments (from its Helm release) for `deploy/immich`.

##### Break-glass

- Operator down: the CNPG webhooks use `failurePolicy: Fail` and everything is one `flux-system` Kustomization, so every apply fails. Suspend it (`flux suspend kustomization flux-system -n flux-system`), roll the operator back with `helm -n cnpg-system rollback cloudnative-pg` (or fix the release), then resume.
- PVC full: raise `spec.storage.size` in `immich/postgres.yaml` (openebs-zfs allows expansion).

### Kubernetes secrets

Encrypted with SOPS using the cluster's age key (`.sops.yaml`); Flux decrypts them when applying (`sops-age` secret). Files must match `*.enc.*`. Only `data` and `stringData` are encrypted.

They are generated from Bitwarden by `cluster-secrets-build`, one namespace at a time. The secretspec profile `dijkstra-<ns>` is the Secret `secrets` of namespace `<ns>`, written to `clusters/dijkstra/secrets/<ns>.enc.yaml`. The profile `dijkstra` itself is cluster-level (the age key) and is never rendered.

1. Add the key to `[profiles."dijkstra-<ns>"]` in `secretspec.toml`, e.g. `API_KEY`. Only `[a-z0-9_]` keys are allowed (case aside), and two keys must not differ only by case.
2. Create the Bitwarden item titled `infra/dijkstra-<ns>/API_KEY` (secretspec escapes a `/` in a profile name, so namespace profiles use `-`).
3. Run `cluster-secrets-build` and commit `clusters/dijkstra/secrets/`.
4. Reference it in the manifests as secret `secrets`, key `api_key` (keys are lowercased).

The namespace must be created by a `kind: Namespace` manifest in `clusters/dijkstra`. The script decrypts with the cluster key from Bitwarden (`FLUX_SOPS_AGE_KEY`), rewrites only files whose content or age recipient changed, lists each file as `new`, `changed`, `stale` or `unchanged` and never prints values. A key declared in the profile but missing from Bitwarden, or with an empty value, fails; Bitwarden items the profile does not declare are ignored. `cluster-secrets-build --check` writes nothing and exits 1 if any file is out of date; it needs Bitwarden and the cluster private key, so it runs only where those are available. To drop a namespace's secrets, remove its profile: the next run deletes the file.

To rotate the cluster key: put the new age public key in the `&dijkstra_cluster` anchor of `.sops.yaml` and the new private key in the Bitwarden item `FLUX_SOPS_AGE_KEY`, delete `clusters/dijkstra/secrets/*.enc.yaml` (the script refuses to run against files encrypted to the old key), run `cluster-secrets-build`, deploy `triss` (`host-deploy triss`, `flux-sops-age` applies the new key), then commit and push.
