provider "cloudflare" {}

locals {
  tunnel_id = "3d9f6e79-4dfb-4349-8407-68d3d1047eff"
}

resource "cloudflare_zone" "nkmrk" {
  account = {
    id = "cfa920ef5f3883a476b0d88cce05813c"
  }
  name = "nkmrk.com"
  type = "full"

  lifecycle {
    prevent_destroy = true
  }
}

resource "cloudflare_dns_record" "vpn_a" {
  zone_id = cloudflare_zone.nkmrk.id
  name    = "vpn.nkmrk.com"
  type    = "A"
  content = oci_core_instance.triss.public_ip
  ttl     = 1
  proxied = false

  lifecycle {
    prevent_destroy = true
  }
}

resource "cloudflare_dns_record" "wildcard_cname" {
  zone_id = cloudflare_zone.nkmrk.id
  name    = "*.nkmrk.com"
  type    = "CNAME"
  content = "${local.tunnel_id}.cfargotunnel.com"
  ttl     = 1
  proxied = true

  lifecycle {
    prevent_destroy = true
  }
}
