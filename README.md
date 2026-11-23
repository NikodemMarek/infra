# infra

Nix flake monorepo: NixOS hosts, their packages, and the manifests of the k3s cluster `dijkstra` that some of those hosts form. Hosts are impermanent (root is wiped on boot). Secrets live in Bitwarden Secrets Manager: hosts get them as sops files built at deploy time, the cluster through External Secrets Operator. Flux reconciles the cluster. Agent rules are in `AGENTS.md`. Run the commands below from the repo root inside the dev shell.

## Layout

```
flake.nix              inputs, nixosConfigurations, deploy-rs nodes, packages
devenv.nix             dev shell: tools and scripts
secretspec.toml        inventory of secrets: a profile per host, plus dijkstra, dijkstra-velero, tailscale, ci, tofu
.sops.yaml             age recipients, one per host
host/<name>/           per-host config, hardware, ssh_host_ed25519_key.pub
host/features/         default.nix, global/ (always on), optional/ (k3s, tailscale, docker, wolf, ...), disko/
modules/host/          own NixOS modules: persist, secrets, eso-bws-token, agents, battery-notifier
pkgs/                  wrapped packages and own tools
clusters/dijkstra/     Kubernetes manifests, reconciled by Flux
tofu/                  OpenTofu: Cloudflare, Tailscale, Oracle Cloud
.github/               CI (hosts build, deploy, tofu); renovate.json: weekly update PRs
```

## Hosts

| Host | Arch | Role |
|------|------|------|
| `yenn` | x86_64 | laptop (Hyprland), LUKS + btrfs; `host-switch`, not a deploy node |
| `geralt` | x86_64 | headless game streaming (Wolf + Moonlight), docker, libvirt; usually off |
| `roach` | x86_64 | k3s agent |
| `regis` | x86_64 | k3s agent, encrypted ZFS (`rpool`, data pool `tank`), NFS server, Garage (S3 for Velero) |
| `triss` | aarch64 | k3s server, Oracle Cloud instance (qemu guest) |
| `alp` | x86_64 | installer ISO (`mkiso`), not in `nixosConfigurations` |

`geralt`, `regis`, `roach` and `triss` are deploy-rs nodes: ssh as `maintenance`, passwordless sudo only for deploy-rs activation. Break-glass for `triss`: `ssh maintenance@141.147.2.187`; there `k3s.yaml` is root-only (`sudo kubectl`).

`geralt` runs [Wolf](https://github.com/games-on-whales/wolf) (`host/features/optional/wolf.nix`, state `/etc/wolf`; the pairing PIN URL is in `docker logs -f wolf`). Wake it with `wake-geralt` on `yenn` (`.link` changes apply only when the NIC appears: reboot once after deploying them). It suspends after 30 min without a stream or SSH login (`host/geralt/idle-suspend.nix`, `journalctl -u idle-suspend`); pause: `sudo systemd-run --unit=stay-awake systemd-inhibit --what=sleep sleep 4h` (cancel: `sudo systemctl stop stay-awake`).

### geralt hardware

- CPU: AMD Ryzen 9 3900X (12C/24T), stock AMD Wraith Prism RGB cooler
- Motherboard: Gigabyte B450M DS3H (AM4), BIOS F41
- RAM: 32 GB, 2x Kingston FURY Beast 16 GB DDR4-3600 CL18 (KF3600C18D4/16GX) in DIMM 1 of channels A and B; 2 of 4 slots used, max 128 GB
- GPU: Gigabyte Radeon RX 5500 XT OC 4G (GV-R55XTOC-4GD, Navi 14)
- SSD: Crucial P1 1 TB NVMe (CT1000P1SSD8)
- Ethernet: onboard Realtek RTL8111/8168 1 GbE, Wake-on-LAN, MAC `b4:2e:99:a4:86:1c`
- WiFi: TP-Link Archer T2U Nano USB (RTL8811AU, driver `rtw88_8821au`)
- PSU: Corsair RM750x (unverified)

### yenn hardware

- Model: Framework Laptop 13 (AMD Ryzen AI 300 Series), board FRANMGCP05, BIOS 03.05 (2025-10-30)
- CPU: AMD Ryzen AI 5 340 (6C/12T), iGPU Radeon 840M
- RAM: 16 GB, 1x ADATA 16 GB DDR5-5600 SODIMM (AD5S560016G-SFW) in channel B; 1 of 2 slots used, max 64 GB
- SSD: WD_BLACK SN7100 1 TB NVMe
- Display: 13.5" 2256x1504
- WiFi/Bluetooth: MediaTek MT7925 (`mt7925e`)
- Battery: 61 Wh (FRANGWA)
- Other: Goodix fingerprint reader, Framework webcam module (2nd gen), HDMI expansion card

### regis hardware

- Model: TerraMaster F4-425 Pro (board M-TWLN01), BIOS BJFX-TWLX-108 (2026-02-02)
- CPU: Intel Core i3-N305 (8C/8T), iGPU Intel UHD (`i915`)
- RAM: 8 GB, 1x Shenzhen Zhongteng 8 GB DDR4-3200 SODIMM (no part number) in Controller0-ChannelA-DIMM0; 1 of 2 slots used, max 64 GB
- SSD: Samsung 990 PRO 1 TB NVMe (`rpool`)
- HDD: 2x Seagate IronWolf Pro 18 TB (ST18000NE000), ZFS mirror (`tank`); 2 of 4 bays used
- Ethernet: 2x onboard Realtek RTL8126 5 GbE (out-of-tree `r8126`), MACs `6c:bf:b5:05:77:88`, `6c:bf:b5:05:77:89`

## Dev shell

`devenv` is loaded by direnv, or run `devenv shell`. Scripts that build host secrets need `BWS_ACCESS_TOKEN` (machine account `ci-hosts`, or a local token).

| Script | What it does |
|--------|--------------|
| `host-switch [action]` | `nixos-rebuild <action>` (default `switch`) of the current host |
| `host-deploy <node...>` | deploy-rs to the given nodes |
| `install-remote <host> <user@ip>` | first install with nixos-anywhere + disko |
| `secrets-build <outdir> <host...>` | build encrypted sops files from the project `hosts` |
| `mkiso` / `writeiso /dev/XXX` | build the `alp` installer ISO / write it to a device |
| `tofu <args>` | OpenTofu in `tofu/`; `fmt`, `version`, `validate`, `init -backend=false` need no token |

## Installing a new host

1. Add `host/<name>/default.nix`, an entry in `nixosConfigurations` (and `deploy.nodes` for remote deploys) and a disko layout from `host/features/disko/` (custom: `host/regis/storage.nix`).
2. Generate the ssh host key, which is also the host's sops identity:
   ```sh
   ssh-keygen -t ed25519 -N "" -f host/<name>/ssh_host_ed25519_key
   ssh-to-age < host/<name>/ssh_host_ed25519_key.pub
   ```
   Commit only the `.pub`. Put the private half in the project `hosts` as `<name>/HOST_SSH_ED25519_PRIV` and delete the local file after installing (git-ignored).
3. Add the age key to `.sops.yaml` (an anchor under `&hosts` and a `host/<name>/.+$` creation rule) and a `[profiles.<name>]` to `secretspec.toml`; create the secrets in the project `hosts` as `<name>/<KEY>`.
4. Boot the target from the ISO (`mkiso`, `writeiso`). It joins tailscale with an auth key from `host/alp/key` (git-ignored, create it in the Tailscale console) and accepts `yenn`'s user key over ssh.
5. A host with a ZFS pool needs `host/<name>/disk.key` (git-ignored; `regis`: `dd if=/dev/urandom of=host/regis/disk.key bs=32 count=1`).
6. `install-remote <name> root@<ip>` builds the secrets, copies the host key and the disk key to the new disk and runs nixos-anywhere.
7. Servers (importing `tailscale-server.nix`) join the tailnet on first boot as `tag:server` with `tailscale/TAILSCALE_AUTH_KEY` (project `hosts`). A k3s node is installed without `k3s.nix` first; then add its tailnet IP (`tailscale ip -4`) to `ips` in `host/features/optional/k3s.nix`, import it and `host-deploy` it.

## Updating hosts

```sh
host-switch            # the machine you are on (yenn)
host-switch test       # any nixos-rebuild action
host-deploy geralt roach
```

CI deploys on merge (`.github/workflows/deploy.yml`) and builds every host on PRs (`hosts.yml`). Offline hosts are skipped; re-running an old run redeploys that old commit. The deploy job reads the `ci` profile (ssh key, Tailscale OAuth client with scope `auth_keys`, tag `tag:ci`) from the project `hosts`; the GitHub environment `deploy` (branch `main`) holds `BWS_ACCESS_TOKEN` of `ci-hosts`.

## Secrets

Modules never talk to sops directly:

- `modules/host/secrets.nix` defines `config.secrets.<name>` with `owner`, `group`, `mode`, `neededForUsers` and `path` (default `/run/secrets/<name>`). Consumers read only `.path`. `profile` (default: the host's name) names the profile of a secret that lives elsewhere, e.g. `dijkstra`.
- `host/features/global/sops.nix` is the only place that knows about sops-nix; it reads `<host>.yaml` from the `secrets` flake input. The age identity is the host's ssh key under `/persist/data/etc/ssh/`.
- `secretspec.toml` is the inventory: the profile named after the host lists its secrets as uppercased names; evaluation fails if modules and profile diverge (names match `^[a-z0-9_]+$`). `ci` and `tofu` are used by no host.
- Values live in the project `hosts`, named `<profile>/<KEY>` (case-insensitive). `tofu` has its own project with plain key names.

`secrets-build <outdir> <host...>` keeps exactly the declared secrets, checks the ssh private keys against the `.pub` files and the host's age key against `.sops.yaml`, hashes `USERS_*_PASSWORD`, and writes `<outdir>/<host>.yaml` encrypted for the host. Keys marked `composed` in `secretspec.toml` (e.g. `GARAGE_ENV`) are assembled from other keys. The `secrets` flake input is `path:./secrets-stub`; the scripts pass `--override-input secrets path:<outdir>`. By hand:

```sh
secrets-build /tmp/s geralt
nixos-rebuild build --flake .#geralt --override-input secrets path:/tmp/s
```

### Adding a secret

1. Declare it in the module and read its path (`neededForUsers = true` for anything read while users are created):
   ```nix
   secrets.myservice_token = { owner = "myservice"; mode = "0400"; };
   services.myservice.tokenFile = config.secrets.myservice_token.path;
   ```
2. Add `MYSERVICE_TOKEN = { description = "...", required = true }` to `[profiles.<host>]` in `secretspec.toml` for every host importing the module.
3. Create `<host>/MYSERVICE_TOKEN` in the project `hosts`.
4. Check with `nix eval .#nixosConfigurations.geralt.config.secrets --apply builtins.attrNames`, then `host-deploy geralt`.

## Persistence

Root is rolled back on every boot; state survives only in `/persist`, managed by `modules/host/persist.nix` (impermanence).

| Option | Meaning |
|--------|---------|
| `persist.enable` | turn the rollback and bind mounts on |
| `persist.deviceService`, `persist.rootPath` | device unit to wait for (e.g. `dev-nvme0n1p2.device`); device holding the root |
| `persist.isCrypted` / `persist.isZfs` | also wait for `cryptsetup.target` / roll back ZFS instead of btrfs |
| `persist.data.{directories,files}` | state worth keeping, under `/persist/data` |
| `persist.generated.{directories,files}` | state that can be regenerated, under `/persist/generated` (always includes `/var/log`, `/var/lib/nixos`, `/var/lib/systemd`, `/etc/machine-id`) |
| `persist.users.<user>.{data,generated}` | the same, relative to the user's home, e.g. `persist.users.nikodem.data.directories = [ "projects" ]` |

On btrfs the old `root` is kept in `old_roots/<timestamp>` for 30 days; on ZFS the rollback is `zfs rollback -r rpool/root@blank` (snapshot made by disko, `host/regis/storage.nix`).

## Packages

`pkgs/<name>/default.nix` wraps an upstream package together with its config (`symlinkJoin` + `makeWrapper`; see `pkgs/rofi`). To add one: create `pkgs/<name>/`, register it in `pkgs/default.nix` (`<name> = pkgs.callPackage ./<name> {<name> = prev.<name>;};`) and use it as `pkgs.<name>`. Every derivation of that set is a flake package (`nix build .#<name>`; event handlers: `.#event-handlers.<name>`).

Apps that keep state in `$HOME` (`bitwarden-cli`, `gh`, `glab`, `heroic`, `moonlight-qt`, `obsidian`, `prismlauncher`, `zen-browser`) take an optional `dataDir` (`.override {dataDir = "/persist/generated/home/<user>/<app>";}`): `jail` then points their state at a persisted path. `claude` is customised with `.extend {...}`, see `pkgs/claude/README.md`.

### Tools

Own programs (`nix build .#<name>`):

| Package | What it is |
|---------|------------|
| `secrets-build`, `bws-project`, `bws`, `infra-tofu` | Bitwarden Secrets Manager tooling: host sops files, a project as JSON, the `bws` CLI, OpenTofu with the profile `tofu` |
| `knowledge` | CLI of claude's memory repo (`curate` runs on a 6 h user timer) |
| `jj-upload`, `forge` | stacked merge/pull requests from a jj stack; the forge (GitLab, GitHub) client |
| `jw` | jj workspaces at `<repo>.agents/<name>` under `projectsRoot` (default `~/projects`); `jw agent` runs Claude in a new workspace and shpool session (`jw --help`) |
| `event-bus` | events over plain NATS (`$NATS_URL`): `event-pub TYPE` publishes JSON from stdin on `events.TYPE`; `event-subscribe HANDLER SUBJECT...` runs HANDLER per event |
| `event-handlers` | user service, one `event-subscribe` per line of `pkgs/event-handlers/handlers` (subjects stay under `events.`) |
| `agent-inbox`, `agent-task`, `claude-hooks`, `claude-statusline`, `agent-skills` | agent tooling: terminal inbox of Claude prompts, sudo requests and tasks; task capture publishing `task.new`; claude's hooks, status line and skills |
| `matrix-gateway`, `matrix-inbox`, `matrix-gateway-image` | Matrix bridge of the NATS hub (in the cluster) and its client on `yenn`; the image is built into `triss` |
| `tm`, `dictate`, `dictate-type`, `wallpaper` | task list CLI (`$TM_FILE`); microphone to text via the local whisper server; types the dictation into the focused window (SUPER+D toggles); the wallpaper image |
| `jail` | a function: `jail <package> {ENV_VAR = "/abs/path";}` runs every binary with those env vars set |

## External infrastructure

OpenTofu in `tofu/` manages what lives outside the repo. State is in the private bucket `tofu-state` on Oracle Cloud (no locking: one apply at a time); secrets come from the project `tofu`.

- Managed: Cloudflare zone `nkmrk.com` (DNS in `tofu/cloudflare.tf`, mail Worker and Email Routing of `t.nkmrk.com` in `tofu/mail.tf`; not in the dashboard), the tailnet policy and DNS settings, Oracle Cloud `triss` (instance, VCN, subnet, security list, internet gateway, route table), the state bucket and the budget.
- Manual: API tokens and keys, the Tailscale OAuth clients, the cloudflared tunnel, Mullvad, Matrix users, GitHub.

Merging to `main` plans and applies (GitHub environment `tofu`, `BWS_ACCESS_TOKEN` of a read-only machine account). A plan that deletes or replaces anything fails the job: apply it locally after reading the plan, not while the workflow runs.

- To stop managing a resource use `removed { from = ... lifecycle { destroy = false } }`, never just delete the block.
- Drift: `tofu plan -detailed-exitcode` (exit 2 means changes). After a `flake.lock` update that bumps providers (they come from nixpkgs): `tofu init -upgrade`.
- Importing: `import` blocks in `tofu/imports-<provider>.tf`, `tofu plan -generate-config-out=generated.tf` (git-ignored), move the resources into `tofu/<provider>.tf`, apply, delete the imports file.
- Tailscale: edit the policy only in `tofu/tailscale-policy.json` (console edits are reverted; its `tests` block rejects a policy that breaks server connectivity). Break-glass if an apply locks the cluster out: the console's JSON editor (Access controls).
- Oracle Cloud: the public IP of `triss` is ephemeral and not managed. `oci.tf` holds every rule of the default security list and route table (console-added ones are removed); keep TCP 22, UDP 51820 (WireGuard hostPort) and the default route. The budget mails as soon as the tenancy spends anything.

## Cluster

`clusters/dijkstra` is a k3s cluster over tailscale: `triss` is the server (traefik and cloud controller disabled), `roach` and `regis` are agents joining it through its tailscale address. Node config is `host/features/optional/k3s.nix` plus the per-host `services.k3s` blocks; the token is `dijkstra/K3S_TOKEN` in the project `hosts`. `regis` exports NFS (`nfs.nix`) for app volumes.

Flux, bootstrapped in `clusters/dijkstra/flux-system`, watches `main` of the GitHub repo and applies `./clusters/dijkstra`. The domain is the Flux variable `DOMAIN` (`flux-system/kustomization.yaml`); resources with shell `$` carry the `kustomize.toolkit.fluxcd.io/substitute: disabled` annotation.

### Cluster apps

Each app is a directory under `clusters/dijkstra/`; shared: `storageclass.yaml`.

| App | What it is | Notes |
|-----|------------|-------|
| `arrstack` | sonarr, radarr, lidarr, prowlarr, bazarr, qbittorrent (via gluetun/Mullvad), beets, flaresolverr | scaled by KEDA |
| `cloudflare` | cloudflared tunnel | only hostnames in `cloudflared.yaml` are public |
| `cnpg` | CloudNativePG operator | webhooks `failurePolicy: Fail`, see Break-glass |
| `dns` | split-DNS for `nkmrk.com` at `10.43.0.53` (also `<service>.<namespace>.svc.cluster.local`) | |
| `immich`, `navidrome` | photos; Navidrome with `audiomuse-ai` | databases `immich-postgres`, `audiomuse-postgres` (see Backups); AudioMuse-AI settings live in its own DB (web UI) |
| `mail-hook` | mail to `t.nkmrk.com` (Cloudflare Email Worker `tofu/mail-worker.js`) as posts in `#mail` | `mail-hook/mail_hook_token` (project `cluster`) must equal `TF_VAR_mail_hook_token` (project `tofu`): rotate both, then `tofu apply` |
| `matrix` | private non-federated Synapse (`matrix.nkmrk.com`), mautrix bridges (`whatsapp`, `meta`), `gateway` (`matrix-gateway`, bot `@agents`, rooms `#agents`, `#mail`, `#alerts`) | create users: `kubectl -n matrix exec -it deploy/synapse -- register_new_matrix_user -c /config/homeserver.yaml http://localhost:8008` (secret `matrix/registration_shared_secret`); DB `matrix-postgres`; the gateway image is imported into `triss` (`ErrImageNeverPull`: `k3s ctr images import` it); anything on the hub can post into `#agents` as `@agents`: never type secrets there |
| `monitoring` | VictoriaMetrics (single, `victoria-metrics-single`) and kube-state-metrics on `triss`, 1 year retention, PVC `victoria-metrics` (`backup: skip`) | write endpoint `http://100.97.10.25:8428/api/v1/write`, open to the whole tailnet and every pod; VM scrapes the kubelet and cAdvisor of every node, plus any pod or Service annotated `prometheus.io/scrape: "true"` (optional `prometheus.io/port`, `prometheus.io/path`); targets: `kubectl -n monitoring port-forward svc/victoria-metrics-single-server 8428` then `/targets`; Loki (plain manifests, `monitoring/loki`) keeps logs 7 days on a 20Gi volume (`backup: skip`), push endpoint `http://100.97.10.25:3100/loki/api/v1/push`, same open access; every host runs Alloy (`host/features/global/monitoring.nix`): journald, `/var/log/pods` on k3s nodes (query strings stripped from Traefik and Jellyfin lines) and node metrics; Grafana at `grafana.${DOMAIN}` (internal), user `admin`, password `monitoring/grafana_admin_password` in BW project `cluster`; dashboards are vendored JSON in `monitoring/grafana/dashboards` |
| `nats` | hub `yenn` joins as a leafnode over WireGuard; single replica on `triss`, no JetStream | a new client namespace must be added to the NetworkPolicy |
| `omniverse` | git-sync, WebDAV, radicale (CalDAV) | calendar subscriptions: secret `omniverse/calendar_subscriptions`; refresh now: `kubectl -n omniverse create job --from=cronjob/calendar-subscriptions refresh-now` |
| `wireguard` | VPN server on `triss`, egress via Mullvad (`wireguard/mullvad_*` in project `cluster`; without it peers have no internet) | limited peers `10.13.13.2-127` reach only `*.nkmrk.com`, the cluster DNS and the internet; full peers `10.13.13.128-254` also the pod and service networks. New peer: add a `[Peer]` with the next free `10.13.13.N/32` to `wg0.conf`; client `DNS = 10.43.0.53`, `MTU = 1200`, `Endpoint = vpn.nkmrk.com:51820`. `yenn` is a peer (`host/yenn/wireguard.nix`): tunnel down means no internet (`sudo networkctl down wg0` / `up`) |

Others: `cert-manager` (Let's Encrypt wildcard), `cron`, `external-secrets`, `intel-gpu-plugin`, `jellyfin` (with seerr), `keda`, `ollama`, `openebs`, `proxy` (traefik, gateway), `velero`, `webdav` (books).

Internal services (`<name>.nkmrk.com`, gateway `internal` listener, full WireGuard peers only, HTTPS): a new name needs an HTTPRoute on `sectionName: internal` and an entry in the `dns` Corefile. `bazarr` and `prowlarr` go through the KEDA interceptor (`keda/httproute.yaml`), so their host is also listed in their `HTTPScaledObject`.

### Kubernetes secrets

The Secret `secrets` of a namespace comes from the ExternalSecret in `clusters/dijkstra/<ns>/external-secret.yaml`, which reads the project `cluster` through the ClusterSecretStore `bitwarden` (read-only machine account `eso`). BWS secrets are named `<ns>/<key>`, lowercase (the case matters here). The ESO access token is `dijkstra/ESO_BWS_TOKEN` in the project `hosts`, applied by `triss` (`services.eso-bws-token`).

1. Create the secret `<ns>/<key>` in the project `cluster`.
2. Add `- secretKey: <key>` with `remoteRef: {key: <ns>/<key>}` to the `data` of the ExternalSecret.
3. Reference it in the manifests as secret `secrets`, key `<key>`.

Composed values (e.g. the appservice registrations of `matrix`) are rendered in `spec.target.template`. Changes apply within 15 minutes; to apply now (then delete the pod if the app reads the secret at startup):

```sh
kubectl -n <ns> annotate externalsecret secrets force-sync=$(date +%s) --overwrite
```

After rotating the ESO token, deploy `triss` and restart `eso-bws-token.service` there.

A fresh cluster: k3s and `triss` (applies the ESO token), `flux bootstrap`, then `kubectl apply -f` `clusters/dijkstra/cert-manager/` and `clusters/dijkstra/external-secrets/{namespace,helm,sdk-server}.yaml` by hand (Flux fails on the whole tree until the ExternalSecret, ClusterSecretStore and Certificate CRDs exist), wait until their HelmReleases are Ready, then reconcile.

### Nix cache

CI shares a binary cache in the Garage bucket `nix-cache` on `regis`: `http://nix-cache:3902`, read anonymously over Garage's web endpoint, reachable only over the tailnet and the cluster (the workflows map `nix-cache` to `100.100.215.91` in `/etc/hosts`). Paths are signed with `nix-cache-1`; hosts trust it through `host/features/global/nix.nix` (all but `triss`), the workflows through `extra_nix_config` in `hosts.yml` and `deploy.yml`.

- Only `hosts.yml` uploads, from the builds with placeholder secrets on push to `main`, over the S3 API on port 3900. The GitHub environment `nix-cache` (`main` only) holds exactly `NIX_CACHE_SIGNING_KEY`, `NIX_CACHE_ACCESS_KEY_ID` and `NIX_CACHE_SECRET_ACCESS_KEY`, nothing else. The repo secrets `TS_CACHE_OAUTH_CLIENT_ID` and `TS_CACHE_OAUTH_SECRET` are a Tailscale OAuth client (scope `auth_keys`, tag `tag:ci-cache` only). `deploy.yml` only reads.
- Accepted risk: a push to `main` can sign paths that every host trusts, including `yenn` and `roach`. Recovery: generate `nix-cache-2`, replace the public key in all three places and the `NIX_CACHE_SIGNING_KEY` secret (and Bitwarden), empty the bucket, then redeploy every host: `deploy.yml` covers `geralt`, `regis` and `triss`; run `host-switch` on `yenn` and `host-deploy roach`.
- The bucket has a 200 GiB quota. If it is full or broken, empty it with a temporary key; the next push to `main` uploads everything again. The `garage` commands run on `regis`; `aws` runs anywhere on the tailnet with the id and secret that `key create` printed:
  ```sh
  garage key create wipe
  garage bucket allow --read --write nix-cache --key wipe
  export AWS_ACCESS_KEY_ID=<GK...> AWS_SECRET_ACCESS_KEY=<secret>
  nix run nixpkgs#awscli2 -- s3 rm --recursive s3://nix-cache --endpoint-url http://100.100.215.91:3900 --region garage
  garage key delete --yes wipe
  ```
  To recreate it, on `regis` (a new `ci-nix-cache` key goes into the `nix-cache` environment and Bitwarden):
  ```sh
  garage bucket create nix-cache
  garage bucket website --allow nix-cache
  garage bucket set-quotas nix-cache --max-size 200GiB
  garage key create ci-nix-cache
  garage bucket allow --read --write nix-cache --key ci-nix-cache
  ```

### Backups

Velero backs up the cluster daily at 18:00 Europe/Warsaw (schedule `daily-openebs`), keeping each backup 30 days, to Garage on `regis` (`host/regis/garage.nix`): a single-node S3 store on `tank/garage`, bucket `velero` at `http://100.100.215.91:3900`, reachable only over `tailscale0` and `cni0`. Velero and Garage share one key pair: `dijkstra-velero/ACCESS_KEY_ID` and `SECRET_ACCESS_KEY` in the project `hosts` (Garage), copied as `velero/access_key_id` and `velero/secret_access_key` in the project `cluster` (rotate both).

- Backed up: every namespace except `kube-system`, `velero` and `flux-system`, without Secrets (they come from Bitwarden), and pod volumes through kopia, except `local-storage` (media on tank), `nfs`, emptyDir and PVCs labelled `backup: skip` (`velero/volume-policy.yaml`).
- The beets library (`library.db`) is on its own PVC `beets-data`: `velero restore` that PVC, after deleting a leftover `library.db-journal` next to it (sqlite would roll it back onto the restored file).
- Grafana alerts post to Matrix `#alerts` (`monitoring/grafana/helm.yaml`), among them a stale (BackupStale) or failed (BackupFailed) Velero backup.

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

- Delete only the backed-up `openebs-*` PVCs. Keep the `backup: skip` and static `local-storage`/`nfs` ones: Velero restores their PV as is, so a deleted one stays Pending.
- A PVC shared by several Deployments (`arrstack-config-pvc`, `omniverse-pvc`) is restored through only one of their pods; the others start on the empty volume and write to it first. Restore those with kopia (below).
- Don't delete the namespace: the static media PVs would go Released with a stale `claimRef` and the restored PVCs wouldn't bind.
- Arrstack apps are scaled by KEDA: first `kubectl -n arrstack delete httpscaledobject --all` (Flux is suspended, so it doesn't recreate them).
- The replaced volumes stay as Released PVs (Retain). Once the app works, `kubectl delete pv <pv>`, then `kubectl -n openebs delete zfsvolume <pv>` (destroys the dataset; openebs-zfs) or remove `/var/openebs/local/<pv>` on `triss`.

##### Restore a volume with kopia

For shared volumes, or when a Velero restore fails:

1. Suspend Flux (it resets `replicas`), scale the app to 0, and delete its HTTPScaledObject if KEDA scales it:
   ```sh
   flux suspend kustomization flux-system -n flux-system
   kubectl -n <namespace> scale deploy/<app> --replicas=0
   ```
2. On the node holding the volume, connect read-only to the namespace's repository (Velero's static default password, secret `velero/velero-repo-credentials`, works without the cluster):
   ```sh
   sudo nix run nixpkgs#kopia -- repository connect s3 --endpoint 100.100.215.91:3900 --disable-tls --bucket velero --prefix kopia/<namespace>/ --region garage --access-key <GK...> --secret-access-key <secret> --password static-passw0rd --readonly
   ```
3. Find the snapshot ID (without the cluster: `kopia snapshot list --all`):
   ```sh
   kubectl -n velero get podvolumebackups -l velero.io/backup-name=<backup> -o custom-columns=VOL:.spec.volume,POD:.spec.pod.name,SNAP:.status.snapshotID
   ```
4. Find the volume's directory `<path>` and a place `<aside>` for its current contents outside it (`<pv>` is the PVC's `spec.volumeName`):
   - `triss`: `<path>` is `/var/openebs/local/<pv>`, `<aside>` is `/var/openebs/local/<pv>.pre-restore`.
   - `regis`: mount the dataset (unmounted while the app is down); `<path>` is `/mnt/restore`, `<aside>` is `/mnt/openebs/pre-restore-<pv>` (outside the dataset so its quota doesn't count both copies):
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

#### Postgres databases

Three CloudNativePG clusters hold app databases; live PGDATA is not a consistent backup, so a pre-hook writes a `pg_dump -Fc` to the PVC root before Velero copies it. Restore only from that dump, never with `velero restore` of the namespace (it would recreate operator-owned objects).

| Cluster | Namespace | Database / owner | Dump | App |
|---|---|---|---|---|
| `immich-postgres` | `immich` | `immich` | `immich.dump` | `deploy/immich` |
| `audiomuse-postgres` | `navidrome` | `audiomuse` | `audiomuse.dump` | AudioMuse-AI (Helm release) |
| `matrix-postgres` | `matrix` | `synapse` | `synapse.dump` | Synapse |

##### Restore a database

Shown for Immich; substitute the row above for another database. Run from the workstation. `<pod>` is the primary: `kubectl -n immich get cluster immich-postgres -o jsonpath='{.status.currentPrimary}'` (usually `immich-postgres-1`).

1. Make sure the Cluster from git is healthy (`kubectl -n immich get cluster immich-postgres`); if missing, let Flux recreate it (a hand-written bootstrap must set `database: immich` and `owner: immich`, else CNPG creates an empty `app` database).
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


##### Break-glass

- Operator down: the CNPG webhooks use `failurePolicy: Fail` and everything is one `flux-system` Kustomization, so every apply fails. Suspend it (`flux suspend kustomization flux-system -n flux-system`), roll the operator back with `helm -n cnpg-system rollback cloudnative-pg` (or fix the release), then resume.
- PVC full: raise `spec.storage.size` in the cluster's `postgres.yaml` (openebs-zfs allows expansion).
