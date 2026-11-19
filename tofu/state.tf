resource "oci_objectstorage_bucket" "tofu_state" {
  compartment_id = local.tenancy_ocid
  namespace      = "frsqgnv1edf0"
  name           = "tofu-state"
  access_type    = "NoPublicAccess"
  storage_tier   = "Standard"
  versioning     = "Enabled"

  lifecycle {
    prevent_destroy = true
  }
}
