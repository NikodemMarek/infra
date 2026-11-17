# infra

README.md is the reference (sections Dev shell, Updating hosts, Secrets, Cluster, Kubernetes secrets, Backups): read the section you need instead of guessing. This file holds the rules for working in the repo.

## Repo

- Keep the repo generic. Workflow-specific config (e.g. agent-inbox tabs keyed on handler kinds) belongs in dotfiles.
- Manifests carry no explanatory comments: the why goes in the commit message. Elsewhere comment only a non-obvious why.
- No test suites for small glue scripts (wrappers, handlers, loops). Libraries and CLIs (Rust, Python) get tests.
- One-time data migrations are done by hand (`kubectl exec`/`cp`), not with initContainers, hooks or jobs in the repo.
- The cluster is `clusters/dijkstra` and Flux applies `main`. Never edit `gotk-*.yaml`.

## Check

Pick the narrowest check that proves the change:

- One package: `nix build .#<pkg>` (runs its tests during the build).
- A host: `nix build --dry-run .#nixosConfigurations.<host>.config.system.build.toplevel --override-input secrets path:<dir>`, where `<dir>` holds an (empty) `<host>.yaml`. `secrets-stub` has no files, so evaluation fails without the override. CI (`hosts.yml`) builds every host on PRs.
- An image triss imports (`shani`, `matrix-gateway`): `nix build .#nixosConfigurations.triss.pkgs.<name>-image`. aarch64 builds on `yenn` through binfmt.
- The cluster: `flux build kustomization flux-system --path ./clusters/dijkstra --kustomization-file ./clusters/dijkstra/flux-system/gotk-sync.yaml --dry-run`. `kustomize build clusters/dijkstra` fails: there is no `kustomization.yaml` there, Flux generates one. Against the live API: `kubectl apply --dry-run=server -f <file>`.
- OpenTofu: `tofu fmt`, `tofu validate` and `tofu init -backend=false` run without a token.

Rules:

- `nix eval` is blocked in agent sessions. Use the dry-run build, or `nix repl` with `:lf .` for single values (e.g. `nixosConfigurations.triss.pkgs.shani-image.imageTag`).
- Build in a jj workspace (no `.git`, so nix sees the files as they are). In the colocated main checkout new files are invisible to `nix build .#` until `git add`ed. Never use `path:.#` there: it copies the git-ignored plaintext keys (`host/alp/key`, `host/*/ssh_host_ed25519_key`, `host/*/disk.key`) into the store.
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
- Cluster: Flux applies merged changes (`flux reconcile kustomization flux-system -n flux-system --with-source` to hurry). The Kustomization has no health checks, so check the pods yourself.
- Images built into triss (`services.k3s.images`, `imagePullPolicy: Never`, tag = the image's `imageTag`): put the new tag in the deployment, `host-deploy triss` from a checkout that contains every image-changing PR, then merge. Merging first leaves the pod in `ErrImageNeverPull`. Changes to anything inside the `shani` image (e.g. `pkgs/claude`, `pkgs/jw`, `pkgs/event-handlers`, `pkgs/shani`) change its tag. Remove old images by name; never `crictl rmi --prune`, it drops the imported ones.
- Restart by hand after changing:
  - a ConfigMap mounted with `subPath`: delete the pod. Avoid `rollout restart` on Flux deployments: a second rollout follows about 30 s later.
  - the NATS hub's leafnodes config: delete pod `nats-0`.
  - the zfs-localpv `zfs.bin` setting: delete the `openebs-zfs-localpv-node` pods in `openebs`.
- OpenTofu: edit `tofu/`; CI plans and applies on merge and fails a plan that deletes or replaces anything. Never apply a plan with `-/+` or destroys you did not intend.
- Ad hoc Velero backup: `velero backup create <name> --from-schedule daily-openebs`. A bare `backup create` copies no pod volumes.
- Some settings live only in app UIs, databases or hand-edited files (Jellyfin, the *arr apps, Lidarr plugins, Immich, AudioMuse-AI, Wolf on `geralt`) and are not in git. Do not change them blind.
