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
modules/host/          own NixOS modules: persist.nix, secrets.nix, battery-notifier.nix, flux-sops-age.nix, event-router.nix (the router user service, the knowledge curator timer and the shpool user service)
pkgs/                  wrapped packages (package + bundled config) and own tools (pkgs/agent-skills/skills: Agent Skills, shared by claude and agent-skills)
overlays/              exposes pkgs/ (the wrapped packages, shadowing the nixpkgs ones, and the tools), flake inputs as packages, deploy-rs
clusters/dijkstra/     Kubernetes manifests, reconciled by Flux
```

## Hosts

| Host | Arch | Role |
|------|------|------|
| `yenn` | x86_64 | laptop/desktop (Hyprland), LUKS + btrfs. Not a deploy node, rebuilt with `host-switch` |
| `geralt` | x86_64 | headless game streaming (Wolf + Moonlight), docker, libvirt; usually off, woken over LAN |
| `roach` | x86_64 | k3s agent, NFS server (media) |
| `regis` | x86_64 | k3s agent, encrypted ZFS (`rpool` + data pool `tank`), NFS, Garage (S3 for Velero) |
| `triss` | aarch64 | k3s server (control plane), qemu/virtio guest |
| `alp` | x86_64 | installer ISO, not in `nixosConfigurations` (see `mkiso`) |

`geralt`, `roach`, `regis` and `triss` are deploy-rs nodes (`deploy.nodes` in `flake.nix`): ssh as `maintenance`, interactive sudo.

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

`secrets-build <outdir> <host...>` exports each profile with `secretspec`, keeps exactly the secrets the config declares, verifies that ssh private keys match the `.pub` files in the repo and that the host's age key (and `FLUX_SOPS_AGE_KEY`, where a host gets it) matches `.sops.yaml`, hashes `USERS_*_PASSWORD` entries (login passwords) with `mkpasswd`, and writes `<outdir>/<host>.yaml` encrypted for the host (plaintext never touches disk). Keys marked `composed` in `secretspec.toml` (e.g. `K3S_VPN_AUTH`) are assembled from other keys; the inputs are not written.

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
| `knowledge` | knowledge CLI of claude's memory repo: `knowledge submit` queues a note, `curate` (the user timer, every 6 h) merges the notes into the scope files, `skills` drafts skill ideas on demand, `hook` is the session-start/subagent-stop hook; `knowledge route` picks the repo a task belongs in |
| `jj-upload` | pushes a stack of jj changes as stacked merge/pull requests (GitLab, GitHub) |
| `jw` | jj workspaces at `<repo>.agents/<name>` for the repos under `projectsRoot` (default `~/projects`, set with `.override {projectsRoot = ...;}`; `yenn` uses `/persist/data/home/nikodem/projects`), with a fish function and completions; `jw new -r REV` starts on a revision or bookmark; `jw agent [-d] [-r REV] [repo] [--] [prompt]` always runs Claude in a new `s-MMDD-HHMMSS` workspace (on `trunk()` or REV) in a shpool session of the same name, and fails for a non-jj repo; outside a repo and with a terminal, `jw agent "<task>"` picks the repo the task fits (`knowledge route`; the picker if unsure), without one it fails with "no repo" |
| `wallpaper` | the wallpaper image (the store path is the file), used by stylix, hyprpaper and hyprlock |
| `claude-hooks` | hook commands of claude: write/bash guards, architect memory autocommit, event routing, sudo askpass and the permission hook (lets agent-inbox answer permission requests and questions; `CLAUDE_INBOX_WAIT=0` turns that off), the Stop hook (in event-started sessions posts one `claude.waiting` event per turn; the inbox keeps the latest per session, key = session id), queued follow-up prompts of event sessions (`$CLAUDE_EVENT_SESSION_DIR/queue/*.md`; the Stop/UserPromptSubmit hooks hand Claude only their paths), jj workspaces for agents |
| `claude-statusline` | claude's status line |
| `agent-skills` | `pkgs/agent-skills/skills/` in the Agent Skills format, to link into the skills directory of other tools (OpenCode, ...) |
| `event-router` | local HTTP inbox (`event-router emit <type>`) that runs a handler per event type; runs as a systemd user service via the `services.event-router` module. Handlers are the `handlers` argument, extend it with `.override`. Event types listed in the `inbox` argument (prefixes) are also queued in memory: `GET /inbox`, `DELETE /inbox/<id>`; an event with a non-empty string `key` replaces the queued item of the same type and key |
| `agent-inbox` | terminal UI over the router's queue (`claude.*` and `sudo.*` events; only sessions started by `jw agent` or the GitLab event handler, i.e. with `CLAUDE_ORIGIN_FILE`, hand-started ones get the local dialog only, sudo prompts always route): rows show the MR or task title, the detail pane the MR description or task prompt, `o` opens the MR, `f` cycles the filter tabs (package arg `tabs`, a list of `{name; kinds ? null;}`): list, inspect, dismiss, answer sudo password requests (written to the askpass FIFO, never through the router) and Claude permission prompts and questions (written to the permission hook's FIFO), attach to an item's shpool session in a zellij tab (`a`); also `list`, `reply <id>`, `dismiss <id>` |
| `event-handlers.notify` | router handler: desktop notification with an "Open" action (flake: `event-handler-notify`; `notify-critical` is its critical-urgency variant) |
| `event-handlers.claude` | GitLab event → Claude session: opens a shpool session (`shpool attach <name>`, 1-day TTL; the shpool user service comes from `services.event-router`) with Claude in the project's checkout under `$HOME/<projectsDir>`; if the context tool returns the optional `session` field, it continues the MR's one conversation (resume), and a live session only gets a "queued" notification and picks the update up through its hooks; `contextTool` (a `gitlab-context prepare` compatible package, default none: plain Claude in the main checkout), `extraPackages`, the `gitlab.*` routes and the watcher are supplied by the work layer (flake: `event-handler-claude`) |

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

Each app is a directory: `arrstack` (sonarr, radarr, lidarr, prowlarr, bazarr, qbittorrent, beets, flaresolverr), `cloudflare` (tunnel), `cnpg` (CloudNativePG operator), `cron`, `immich`, `intel-gpu-plugin` (exposes regis's iGPU as `gpu.intel.com/i915`), `jellyfin`, `keda`, `navidrome` (with audiomuse-ai), `ollama`, `omniverse` (git-sync, webdav, radicale), `openebs`, `proxy` (traefik, gateway), `shani`, `tailscale` (operator), `velero`. Shared: `storageclass.yaml`.

AudioMuse-AI (`navidrome/audiomuse-ai`) keeps its settings (Navidrome login, auth/API token, AI provider) in its own database, set in its web UI at `http://audiomuse-ai/` (Tailscale). Navidrome's `audiomuseai` plugin is configured in Navidrome's UI with that token.

Radicale (`omniverse/radicale`) serves CalDAV at `http://radicale.tail5874bc.ts.net/`, user `nikodem`. Its data is the omniverse `dav/` directory, versioned by git-sync except what the `omniverse-gitignore` ConfigMap excludes (the `sub-*` subscription calendars and Radicale runtime files). `dav/` is not served over the omniverse WebDAV because Radicale is its only writer: use CalDAV (DAVx⁵ on Android).

Calendar subscriptions are listed in the secret `CALENDAR_SUBSCRIPTIONS` (profile `dijkstra-omniverse`, rebuild with `cluster-secrets-build`), one `<name> <url> [<display name>]` per line: `<name>` is lowercase letters, digits and dashes, starting with a letter or digit; `<url>` is `http(s)://` or `webcal://`; the display name (may contain spaces) defaults to `<name>`. All other lines are ignored. The CronJob `calendar-subscriptions` refreshes them daily at 04:00 Europe/Warsaw into read-only `sub-<name>` calendars. Run it now with:

```bash
kubectl -n omniverse create job --from=cronjob/calendar-subscriptions refresh-now
```

To remove a subscription, delete its line, then delete the calendar (read-only over CalDAV):

```bash
kubectl -n omniverse exec deploy/radicale -- rm -r /data/collection-root/nikodem/sub-<name>
```

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

##### Break-glass

- Operator down: the CNPG webhooks use `failurePolicy: Fail` and everything is one `flux-system` Kustomization, so every apply fails. Suspend it (`flux suspend kustomization flux-system -n flux-system`), roll the operator back with `helm -n cnpg-system rollback cloudnative-pg` (or fix the release), then resume.
- PVC full: raise `spec.storage.size` in `immich/postgres.yaml` (openebs-zfs allows expansion).

### Kubernetes secrets

Encrypted with SOPS using the cluster's age key (`.sops.yaml`); Flux decrypts them when applying (`sops-age` secret). Files must match `*.enc.*`. Only `data` and `stringData` are encrypted.

They are generated from Bitwarden by `cluster-secrets-build`, one namespace at a time. The secretspec profile `dijkstra-<ns>` is the Secret `secrets` of namespace `<ns>`, written to `clusters/dijkstra/secrets/<ns>.enc.yaml`. The profile `dijkstra` itself is cluster-level (the age key) and is never rendered.

1. Add the key to `[profiles."dijkstra-<ns>"]` in `secretspec.toml`, e.g. `API_KEY`. Only `[a-z0-9_]` keys are allowed (case aside), and two keys must not differ only by case. As for hosts, a key that only feeds a `composed` key is an input and is not written.
2. Create the Bitwarden item `API_KEY` in the folder `infra/dijkstra-<ns>` (i.e. `infra/dijkstra-<ns>/API_KEY`; secretspec escapes a `/` in a profile name, so namespace profiles use `-`).
3. Run `cluster-secrets-build` and commit `clusters/dijkstra/secrets/`.
4. Reference it in the manifests as secret `secrets`, key `api_key` (keys are lowercased).

The namespace must be created by a `kind: Namespace` manifest in `clusters/dijkstra`. The script decrypts with the cluster key from Bitwarden (`FLUX_SOPS_AGE_KEY`), rewrites only files whose content or age recipient changed, lists each file as `new`, `changed`, `stale` or `unchanged` and never prints values. A key declared in the profile but missing from Bitwarden, or with an empty value, fails; Bitwarden items the profile does not declare are ignored. `cluster-secrets-build --check` writes nothing and exits 1 if any file is out of date; it needs Bitwarden and the cluster private key, so it runs only where those are available. To drop a namespace's secrets, remove its profile: the next run deletes the file.

To rotate the cluster key: put the new age public key in the `&dijkstra_cluster` anchor of `.sops.yaml` and the new private key in the Bitwarden item `FLUX_SOPS_AGE_KEY`, delete `clusters/dijkstra/secrets/*.enc.yaml` (the script refuses to run against files encrypted to the old key), run `cluster-secrets-build`, deploy `triss` (`host-deploy triss`, `flux-sops-age` applies the new key), then commit and push.
