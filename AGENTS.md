# infra

README.md is the reference (sections Layout, Hosts, Dev shell, Installing a new host, Updating hosts, Secrets, Persistence, Packages, External infrastructure, Cluster, Backups; Kubernetes secrets and Nix cache are under Cluster): read the section you need instead of guessing. This file holds the rules for working in the repo.

## Repo

- Keep the repo generic. Workflow-specific config belongs in dotfiles.
- Manifests carry no explanatory comments: the why goes in the commit message. Elsewhere comment only a non-obvious why.
- No test suites for small glue scripts (wrappers, handlers, loops). Libraries and CLIs (Rust, Python) get tests.
- One-time data migrations are done by hand (`kubectl exec`/`cp`), not with initContainers, hooks or jobs in the repo.
- Config files mounted into pods go through `configMapGenerator` in the directory's `kustomization.yaml` (as in `mail-hook`), so a change rolls the pod.
- The cluster is `clusters/dijkstra` and Flux applies `main`. Never edit `gotk-*.yaml`.
- The default checkout can lag `trunk()`: read code from a `trunk()` workspace or `jj file show -r 'trunk()' <path>`.
- Keep the README per-host hardware sections (`geralt`, `yenn`, `regis`: parts, slots, BIOS, MACs) complete when trimming the README.
- Never run or plan `--privileged` containers or pods, not even to verify something (one damaged a host). Use a device plugin (Intel GPU) or explicit devices plus cgroup rules.
- Industry standard over clever repo-local: tofu state lives in a remote backend (S3-compatible object storage), not encrypted in git; GitHub is out of tofu scope. Small cluster glue is a stock upstream image plus a ConfigMap (Redpanda Connect, as in `mail-hook`), not an own nix image that needs a deploy and tag pinning per change.
- Tests use `example.com`, never the real domain; manifests take the real domain from one source (`${DOMAIN}`).
- The projects root differs per host: take it as an option (jw `projectsRoot`, claude `projectsDir`), never hardcode `$HOME/projects`.
- Every session starts through `jw agent` (agents and the task event handler included) or `jw mate` (a supervisor session); `jw agent` always creates a fresh jj workspace on top of `jw new` (start point revision or bookmark, default `trunk()`); no raw shpool/claude launchers, so every session has a jw record. Session status has one source of truth: the jw session record written by the agent hooks, not Claude's registry.
- Stack queries are `jj stack <verb>` (e.g. `jj stack status`), not new `jj upload` verbs; bare `jj stack` is the log alias (`pkgs/jujutsu/config.toml`).
- The event router is the hub for agent/session communication (task dispatch, messages, notifications): producers (quick-capture) only publish an event, the handler does the work.
- Quick-capture popups are a minimal launcher extension (rofi/fuzzel), not a TUI.

## Check

Pick the narrowest check that proves the change:

- One package: `nix build .#<pkg>` (runs its tests during the build).
- A host: `nix build --dry-run .#nixosConfigurations.<host>.config.system.build.toplevel --override-input secrets path:<dir>`, where `<dir>` holds an (empty) `<host>.yaml`. `secrets-stub` has no files, so evaluation fails without the override. A full (non-dry) build or deploy needs `key: x` in `<host>.yaml` for every sops secret the host uses, else the manifest build fails; the empty file is enough for the dry run. CI (`hosts.yml`) builds every host on PRs.
- The `matrix-gateway` image: `nix build .#packages.aarch64-linux.matrix-gateway-image` (aarch64 builds on `yenn` through binfmt). CI (`images.yml`) builds it on PRs.
- The cluster: `flux build kustomization flux-system --path ./clusters/dijkstra --kustomization-file ./clusters/dijkstra/flux-system/gotk-sync.yaml --dry-run`. `kustomize build clusters/dijkstra` fails: there is no `kustomization.yaml` there, Flux generates one. Against the live API: `kubectl apply --dry-run=server -f <file>`.
- OpenTofu: `tofu fmt`, `tofu validate` and `tofu init -backend=false` run without a token.

Rules:

- `nix eval` is blocked in agent sessions. Use the dry-run build, or `nix repl` with `:lf .` for single values (e.g. `nixosConfigurations.triss.config.services.k3s.role`).
- Build in a jj workspace (no `.git`, so nix sees the files as they are). In the colocated main checkout new files are invisible to `nix build .#` until `git add`ed. Never use `path:.#` there: it copies the git-ignored plaintext keys (`host/alp/key`, `host/*/ssh_host_ed25519_key`, `host/*/disk.key`) into the store.
- Lint: `devenv test` runs every git hook (treefmt, statix, deadnix, ruff, shellcheck, clippy) on all files; CI (`lint.yml`) runs it on PRs and fails on any finding.
- Format Nix with `nix run nixpkgs#alejandra`. Check scripts with `bash -n` and `nix run nixpkgs#shellcheck -- -s bash`.
- Cluster: `KUBECONFIG` is `kubeconfig.yaml` (git-ignored) in the root of the `default` jj workspace; the dev shell sets it. `kubectl` is read-only unless asked.
- Hosts: `ssh maintenance@<host>`. Its sudo needs a password and a terminal (only deploy-rs activation is passwordless), so sudo steps go to the user.

## Secrets

- Never read, print or log secret values. They live in Bitwarden Secrets Manager (projects `hosts`, `tofu`, `cluster`), which only the user unlocks (`BWS_ACCESS_TOKEN`). Name the item and the command; the user runs it.
- Host sops files are built by `secrets-build` at deploy time, never committed.
- Host secret (README "Adding a secret"): declare `secrets.<name>` and read only `.path`; add the uppercased key to the host's profile in `secretspec.toml`; the user creates the item `<profile>/<KEY>` in the project `hosts`. Evaluation fails when usage and inventory diverge, so the host dry-run build is the check.
- Cluster secret (README "Kubernetes secrets"): add the key to the namespace's `external-secret.yaml` (ExternalSecret `secrets`); the user creates the item `<ns>/<key>` (lowercase) in the project `cluster`. Manifests read Secret `secrets`, key `<key>`.
- When the user generates a secret, remind them to save it in Bitwarden too.
- Velero backups exclude Secrets: after a restore they come back from Bitwarden through External Secrets.

## Deploy

Agents prepare changes and stacked PRs. Deploying and merging is the user's call unless asked. In a plan, list the user's steps in order.

- Merging to `main` deploys `geralt`, `regis` and `triss` (CI); changes under `tofu/` are applied by CI, the cluster by Flux.
- Hosts: `host-switch` (the current host, `yenn`) or `host-deploy <node>...` (deploy-rs as `maintenance`). A switch does not start or restart user units: `systemctl --user restart <unit>` (e.g. `matrix-inbox`).
- Nothing is deployed by hand, neither by sessions nor by the user: every host, cluster and tofu change goes through the pipeline. When asked to deploy, merge the PR after the checks and watch the run.
- Cluster: Flux applies merged changes (`flux reconcile kustomization flux-system -n flux-system --with-source` to hurry). The Kustomization has no health checks, so check the pods yourself.
- `matrix-gateway`: `images.yml` pushes `ghcr.io/nikodemmarek/matrix-gateway:main` on merge; the deployment pins its digest, which Renovate bumps (or bump it by hand after the run).
- Avoid `rollout restart` on Flux deployments: a second rollout follows about 30 s later.
- Restart by hand after changing:
  - the NATS hub's leafnodes config: delete pod `nats-0`.
  - the zfs-localpv `zfs.bin` setting: delete the `openebs-zfs-localpv-node` pods in `openebs`.
- OpenTofu: edit `tofu/`; CI plans and applies on merge and fails a plan that deletes or replaces anything. Never apply a plan with `-/+` or destroys you did not intend.
- Ad hoc Velero backup: `velero backup create <name> --from-schedule daily-openebs`. A bare `backup create` copies no pod volumes.
- Some settings live only in app UIs, databases or hand-edited files (Jellyfin, the *arr apps, Lidarr plugins, Immich, AudioMuse-AI, Wolf on `geralt`) and are not in git. Do not change them blind. On PVCs (Velero-backed) the current values are:
  - Jellyfin `encoding.xml`: QSV, tone mapping on (VPP off), low-power H264/HEVC on, AV1 encode off.
  - Radarr/Sonarr: 2160p quality definitions max 120 MB/min, preferred 95. Profile "Playable" (id 7): custom formats HDR (-10000), Compact (+100; <= 20 GB movie, <= 7.5 GB episode), AI upscale (-10000); `cutoffFormatScore` 100. The cutoff group "2160p" must hold WEBDL/WEBRip/Bluray-2160p, else Bluray never upgrades.
  - qBittorrent: `max_active_torrents` 2, `dont_count_slow_torrents` on. Dead torrents and Lidarr import failures are cleared by hand (blocklist and re-search); there is no Cleanuparr or CronJob.
  - Lidarr: the webhook "Beets" fires only on `onReleaseImport` and `onUpgrade` (else beets answers 400); run one `RescanFolder` at a time (`UNIQUE TrackFiles.Path`).

## Gotchas

NixOS:

- The `services.nats` module sets `settings.jetstream` itself: disable it with `lib.mkForce false`.
- nats-server 2.14.7 refuses `lame_duck_duration` under 30s. A leafnode remote has no allow list, only deny lists.
- Strings joined raw into a unit's `ExecStart` (e.g. `services.k3s.extraFlags`) go through systemd specifier expansion: write a literal `%` as `%%`.
- `networking.nftables.ruleset` makes `flushRuleset` default to true, so every reload drops the tables other services created (docker, libvirt, kube-proxy, tailscale). Only `k3s.nix` sets `flushRuleset = false`; `tailscale.nix` and `libvirt.nix` declare their tables through `ruleset`, so hosts without k3s (`geralt`, `yenn`) still flush on a switch.

`geralt`:

- Never hibernate (S4): a resumed image oopsed on modprobe and hung sshd until a hard reboot. Idle suspend uses S3.
- Harmless: the iwd segfault on resume and the Wolf xorg crash loop (status 11).
- LAN IP `192.168.18.95` (DHCP, so confirm it with `ssh maintenance@regis ip neigh | grep b4:2e:99:a4:86:1c`). Tailnet down: `ssh -o HostKeyAlias=geralt -J maintenance@regis maintenance@192.168.18.95`.
- `/etc/wolf/cfg/config.toml` is owned by Wolf (the original is kept as `.orig`): re-check it after Wolf upgrades. `WOLF_SKIP_WAYLAND_SOCKET_WAIT=TRUE` is required (wolf#462).

`triss`:

- `/boot` is 98 MB and holds one generation (uncompressed 68 MiB arm64 kernel + ~20 MB xz initrd). systemd-boot deletes the old files before copying the new ones, so a kernel or initrd that outgrows the free space fails the install and leaves no bootable entry: check `df /boot` before kernel bumps. The FilesystemFull alert for it fires below 5 MiB free.

`regis` (8 GB, no swap, the k3s node with the ZFS volumes):

- A failed `regis` switch (and its rollback) restarts Garage, which backs the nix cache: CI jobs running at the same time lose the cache and fail or fall back to building. Re-run them after a failed deploy.
- No zram or swap: zram was tried and dropped as unreliable. Memory is kept in check by `host/regis/memory.nix` (ZFS ARC cap 1 GiB, earlyoom, kubelet `system-reserved` 2Gi and `memory.available` eviction, watchdog and panic-reboot sysctls). Garage (systemd `MemoryHigh` 1G, `MemoryMax` 1536M) and k3s run outside the pod budget.
- Keep heavy pods without node-pinned storage (audiomuse-ai) on `triss`. Check the sum of pod memory requests on regis before adding one: above the allocatable ~5.1 GiB new pods stay Pending.

Cluster:

- OpenEBS zfs-localpv refuses a second pod mount unless the ZFSVolume has `spec.shared: "yes"`. Set it through the StorageClass parameter `shared: "yes"`; StorageClass parameters are immutable, so recreate the StorageClass and patch the existing ZFSVolumes. `thinprovision: "no"` reserves the full PVC size.
- k3s `--vpn-auth=name=tailscale` runs `tailscale up --reset` on every start: use `joinKey=none` once logged in. To join: stop k3s, `tailscale up --auth-key=<single-use tagged key>`, start k3s.
- Velero schedules accept `CRON_TZ=`. fs-backup copies only the volumes of running pods.
- CNPG: the pre-backup `pg_dump -Fc` hook omits per-database `ALTER DATABASE ... SET` settings (Immich `vchordrq.probes`, `search_path`). Record them from `pg_db_role_setting` and re-apply them after a restore.
- Gateway API CRDs come from the upstream bundle in `gateway-api/`, not the Traefik chart: bump them before a Traefik release that needs newer ones. With a missing route CRD Traefik's Gateway provider hangs in cache sync while the pod stays Ready and the HelmRelease healthy, so curl a route after every Traefik bump.
- Flux (server-side apply) cannot switch a live Deployment to `strategy.type: Recreate`: the defaulted `rollingUpdate` has no owner and stays, so the API rejects it and the whole Kustomization fails (`rollingUpdate: null` does not help). Use `RollingUpdate` with `maxSurge: 0`, `maxUnavailable: 1`, or delete the Deployment once.
- Helm v4: before disabling a chart's own Gateway/GatewayClass, ship `helm.sh/resource-policy: keep` in a separate change (adding it in the same change deletes the resource).
- Matrix bridges (mautrix):
  - Wiping a bridge DB forgets the management room: use `!wa` or a fresh DM.
  - mautrix-meta login: the messenger.com cookies from "Copy as cURL".
  - Backfill only covers portals created after it was enabled. WhatsApp history comes only at device link: relink and wipe `/data/whatsapp.db*`.
  - Bump the images about monthly.
- DNS: kube-dns stays separate; edge/WireGuard clients use one dedicated CoreDNS (app `dns`: the home domain, `cluster.local`, Mullvad DoT upstream); `yenn` keeps its own systemd-resolved; MagicDNS serves only the tailnet hosts (`triss`, `regis`, `roach`, `geralt`).
- In pods, `pgrep -f` inside `kubectl exec ... sh -c` matches the `sh` itself: anchor the pattern. Copy files in with `kubectl cp`, not a heredoc into `kubectl exec -i`.
- beets: art jobs can hit the 1Gi memory limit (OOMKilled). ReplayGain runs only on import; backfill with `beet replaygain`.

Sessions:

- jw keeps session records, turn results and queued messages in NATS KV buckets `sessions`, `results`, `queue` on yenn's nats container (`NATS_URL`, default `nats://127.0.0.1:4222`); with NATS down record commands fail and hooks only warn. jw tests must start their own nats-server (`tests/common/nats.rs`) and never use `Records::open()`/`NATS_URL`: on yenn a host `cargo test` otherwise writes into the live buckets. Run them with `NATS_URL=nats://127.0.0.1:1` to prove it.
- The jw job script runs `exec direnv exec . claude ...`, so sessions already have the dev shell env (`KUBECONFIG`): don't export it by hand. `<repo>.agents/*` needs no `direnv allow`; `devenv.nix` edits need a session restart.
- A `jw agent --reports-to` worker is headless: a jw session (record, workspace) run as transient `jw-worker-NAME` user units, one `claude -p` per turn, not in shpool. `jw msg` queues for it, `--interactive` gives the shpool session instead. A worker that ends a turn with open MRs (context from 50k tokens) is `/compact`ed 2 min later unless a message came in, so forge event turns start from a summary (`jw-compact-NAME-SEQ` timer, `jw worker compact`). Interactive sessions can't be compacted this way: `/compact` over the messaging socket arrives as plain text.
- Every session `jw` starts or resumes runs claude with `--permission-mode bypassPermissions` (headless turns too), so the PermissionRequest hook never fires. `jw agent --ask-permissions` (kept in the record as `ask_permissions`) opts out: a headless turn runs `--permission-prompts none` (deny and report), an interactive session gets Claude's settings default. GitLab event sessions are started by `pkgs/event-handlers/claude`, not jw, and keep deny-and-report. `skipDangerousModePermissionPrompt` in `pkgs/claude/settings.nix` stops a detached session blocking on bypass mode's one-time dialog. `crossSessionInbound = "accept"` there lets `jw msg` reach bypass sessions: by default they hold peer messages that attest no sender permission mode (`jw msg` sends none) until the user approves each.
- Claude `/resume` filters by cwd and shows nothing in `<repo>.agents/s-*`: use `jw resume [repo]`.
- SendMessage cannot reach a just-started session: use `jw msg` or the event router.
- An event's `type` is its NATS subject: route an event to a new consumer with a line in `pkgs/event-handlers/handlers` (exactly `<subject> <handler>`, no arguments), never by republishing it on another subject.
- jw's `Record` keeps only the keys it declares: an older jw that writes a record drops newer keys.
- In a jj repo without a remote `trunk()` resolves to `root()`: `add_workspace` refuses it and `jj stack status` calls an empty stack merged, so prune/reap would retire its workspaces. The mate repo aliases `trunk()` to `main`, and kept (mate) sessions are skipped by name.
- `jw agent` passes everything after the repo to the prompt: an option written there is silently prompt text unless `agent.rs` lists it as "options go before the repo".
