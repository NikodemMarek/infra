terraform {
  required_providers {
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.0"
    }
    tailscale = {
      source  = "tailscale/tailscale"
      version = "~> 0.29"
    }
    oci = {
      source  = "oracle/oci"
      version = "~> 9.0"
    }
  }

  backend "s3" {
    bucket = "tofu-state"
    key    = "infra.tfstate"
    region = "eu-frankfurt-1"

    endpoints = {
      s3 = "https://frsqgnv1edf0.compat.objectstorage.eu-frankfurt-1.oraclecloud.com"
    }

    skip_region_validation      = true
    skip_credentials_validation = true
    skip_requesting_account_id  = true
    skip_metadata_api_check     = true
    skip_s3_checksum            = true
    use_path_style              = true
  }
}
