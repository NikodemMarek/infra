---
name: infra
description: Check, secrets and deploy workflow for the infra repo (NixOS hosts, wrapped packages, k3s cluster dijkstra via Flux). Use before building, evaluating or testing anything here, when touching secretspec.toml, Bitwarden items or *.enc.yaml files, and before deploying hosts or merging cluster or image changes.
---

# infra

README.md is the reference (sections Dev shell, Updating hosts, Secrets, Infra, Kubernetes secrets, Backups): read the section you need instead of guessing. This skill only adds what an agent session needs on top of it.

## Check
Pick the narrowest check that proves the change:
- One package: `nix build .#<pkg>` (runs its test suite, if it has one, during the build).
- A host: `nix build --dry-run .#nixosConfigurations.<host>.config.system.build.toplevel --override-input secrets path:<dir>`, where `<dir>` holds an empty `<host>.yaml` for each host (the repo's `secrets-stub` has none, so evaluation fails without the override).
- An image triss imports: `nix build .#nixosConfigurations.triss.pkgs.<name>-image` (tm, shani, matrix-gateway). aarch64 builds on yenn via binfmt.
- The cluster: `flux build kustomization flux-system --path ./clusters/dijkstra --kustomization-file ./clusters/dijkstra/flux-system/gotk-sync.yaml --dry-run`. `kustomize build clusters/dijkstra` fails: there is no kustomization.yaml there, Flux generates one. Against the live API: `kubectl apply --dry-run=server -f <file>`.

Rules:
- `nix eval` is blocked in agent sessions. Use the dry-run build above, or `nix repl` + `:lf .` for single values (e.g. `nixosConfigurations.triss.pkgs.shani-image.imageTag`).
- Build in a jj workspace (no `.git`, so nix sees the files as they are). In the colocated main checkout, new files are invisible to `nix build .#` until `git add`ed, and jj may reset the git index. Never use `path:.#` there: it copies the git-ignored plaintext keys (`host/alp/key`, `host/*/ssh_host_ed25519_key`, `host/*/disk.key`) into the store.
- Format and lint: `nix run nixpkgs#alejandra` for Nix, `bash -n` + `nix run nixpkgs#shellcheck -- -s bash` for scripts (neither tool is in the dev shell). No test suites for small glue scripts. Comments only for a non-obvious why.
- Cluster: `KUBECONFIG=/persist/data/home/nikodem/projects/infra/kubeconfig.yaml kubectl ...`, read-only unless asked.
- Hosts: `ssh maintenance@<host>`. sudo needs a real terminal, so sudo steps go to the user.

## Secrets
- Never read, print or log secret values. They live only in Bitwarden, which only the user unlocks. Name the item title and the command; the user runs it.
- Host secret (README "Adding a secret"): declare `secrets.<name>` and read only `.path`; add the UPPERCASE key to the host's profile in `secretspec.toml`; Bitwarden item `infra/<profile>/<KEY>`. Evaluation fails when usage and inventory diverge, so the host dry-run build is the check.
- Cluster secret (README "Kubernetes secrets"): key in `[profiles."dijkstra-<ns>"]`, item `infra/dijkstra-<ns>/<KEY>`. The user runs `cluster-secrets-build`, and the regenerated `clusters/dijkstra/secrets/<ns>.enc.yaml` goes into the same change. Manifests read Secret `secrets` with the key lowercased. The namespace needs a `kind: Namespace` manifest.
- Profile names never contain `/`. Seed items byte-exact with `secretspec set --profile <P> <KEY> --from-file -`.
- The inputs of `composed` keys are exported to the cluster too.
- Velero backups exclude Secrets: after a restore, Secrets come back from Bitwarden through Flux.

## Deploy
Agents prepare changes and stacked PRs. Deploying and merging is the user's call unless asked. In a plan, list the user's steps in order.
- Hosts: `host-switch` (yenn, local) or `host-deploy <node>...` (deploy-rs as `maintenance`, sudo answered from Bitwarden). A switch does not start or restart user units: `systemctl --user restart <unit>` (e.g. matrix-inbox).
- Cluster: merge to `main` and Flux applies it (`flux reconcile kustomization flux-system -n flux-system --with-source` to hurry). The Kustomization has no health checks, so check the pods yourself.
- Images built into triss (`services.k3s.images`, `imagePullPolicy: Never`, tag = the image's `imageTag`): put the new tag in the deployment, run `host-deploy triss` from a checkout that contains every image-changing PR, then merge. Merging first leaves the pod in `ErrImageNeverPull`. Changes to `pkgs/jw`, `pkgs/event-handlers` or `pkgs/matrix-inbox` change the shani image tag. Remove old images by name; never `crictl rmi --prune`, which drops the imported ones.
- Restart by hand after changing:
  - a ConfigMap mounted via `subPath`: delete the pod. Avoid `rollout restart` on Flux deployments: a second rollout follows about 30 s later.
  - the NATS hub's leafnodes config: delete pod `nats-0`
  - the zfs-localpv `zfs.bin` setting: delete the `openebs-zfs-localpv-node` pods in `openebs`
- Ad hoc Velero backup: `velero backup create <name> --from-schedule daily-openebs`. A bare `backup create` copies no pod volumes.
- One-time data migrations are done by hand (kubectl exec/cp), not with initContainers or hooks in the repo.
- Some settings live only in app UIs, databases or hand-edited files (Jellyfin, the *arr apps, Lidarr/Tubifarry, Immich, Wolf on geralt) and are not in git. Read the project's facts before changing them.
