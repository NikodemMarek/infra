provider "tailscale" {}

resource "tailscale_acl" "policy" {
  acl = file("${path.module}/tailscale-policy.json")
}

resource "tailscale_dns_configuration" "dns" {
  magic_dns          = true
  override_local_dns = false
}
