# home-infra

- Keep this repo generic. Workflow-specific config (e.g. agent-inbox tabs keyed on handler kinds) belongs in dotfiles.
- Manifests carry no explanatory comments: the why goes in the commit message.
- Secrets live in SOPS (`clusters/dijkstra/secrets/*.enc.yaml`, `host/*`) and in Bitwarden. When the user generates one, remind them to save it in Bitwarden too.
- After changing a ConfigMap that is mounted with `subPath`, delete the pod. Avoid `rollout restart` on Flux deployments: a second rollout follows about 30 s later.
