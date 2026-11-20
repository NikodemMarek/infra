{
  apiVersion: "v1",
  kind: "Secret",
  metadata: {
    name: "secrets",
    namespace: $ns,
    annotations: {"kustomize.toolkit.fluxcd.io/prune": "disabled"}
  },
  type: "Opaque",
  data: map_values(@base64)
}
