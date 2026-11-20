variable "mail_hook_token" {
  type      = string
  sensitive = true
}

variable "mail_forward_to" {
  type      = string
  sensitive = true
}

resource "cloudflare_workers_script" "mail_hook" {
  account_id         = cloudflare_zone.nkmrk.account.id
  script_name        = "mail-hook"
  content            = file("${path.module}/mail-worker.js")
  main_module        = "mail-worker.js"
  compatibility_date = "2026-11-01"

  bindings = [
    {
      name = "HOOK_URL"
      type = "plain_text"
      text = "https://mail-hook.${cloudflare_zone.nkmrk.name}/mail"
    },
    {
      name = "HOOK_TOKEN"
      type = "secret_text"
      text = var.mail_hook_token
    },
    {
      name = "FORWARD_TO"
      type = "secret_text"
      text = var.mail_forward_to
    },
  ]
}

resource "cloudflare_email_routing_dns" "t" {
  zone_id = cloudflare_zone.nkmrk.id
  name    = "t.${cloudflare_zone.nkmrk.name}"
}

resource "cloudflare_email_routing_address" "forward_to" {
  account_id = cloudflare_zone.nkmrk.account.id
  email      = var.mail_forward_to
}

resource "cloudflare_email_routing_catch_all" "t" {
  zone_id = cloudflare_zone.nkmrk.id
  name    = "mail-hook"
  enabled = true

  matchers = [
    {
      type = "all"
    },
  ]

  actions = [
    {
      type  = "worker"
      value = [cloudflare_workers_script.mail_hook.script_name]
    },
  ]
}
