locals {
  tenancy_ocid = "ocid1.tenancy.oc1..aaaaaaaa2vaiux5lkc54bkn46n3zjekb536aqv3i66sghoii6uaumvwxl3bq"
}

provider "oci" {
  tenancy_ocid = local.tenancy_ocid
  user_ocid    = "ocid1.user.oc1..aaaaaaaak2wx72vcsjzmylydbig6cmtkanivs3bzzsytra35r7bkiakqfjuq"
  fingerprint  = "36:84:1b:63:33:d3:be:31:21:cc:18:b9:0e:a6:ed:3e"
  region       = "eu-frankfurt-1"
}

resource "oci_budget_budget" "tenancy" {
  compartment_id = local.tenancy_ocid
  display_name   = "tenancy"
  amount         = 1
  reset_period   = "MONTHLY"
  target_type    = "COMPARTMENT"
  targets        = [local.tenancy_ocid]
}

resource "oci_budget_alert_rule" "any_spend" {
  budget_id      = oci_budget_budget.tenancy.id
  display_name   = "any-spend"
  type           = "ACTUAL"
  threshold_type = "PERCENTAGE"
  threshold      = 1
  recipients     = "nmarekpub@proton.me"
  message        = "Oracle Cloud tenancy is spending money"
}

resource "oci_core_default_security_list" "main" {
  manage_default_resource_id = oci_core_vcn.main.default_security_list_id

  egress_security_rules {
    destination = "0.0.0.0/0"
    protocol    = "all"
  }

  ingress_security_rules {
    source   = "0.0.0.0/0"
    protocol = "6"
    tcp_options {
      min = 22
      max = 22
    }
  }

  ingress_security_rules {
    source   = "0.0.0.0/0"
    protocol = "1"
    icmp_options {
      type = 3
      code = 4
    }
  }

  ingress_security_rules {
    source   = "10.0.0.0/16"
    protocol = "1"
    icmp_options {
      type = 3
    }
  }

  ingress_security_rules {
    description = "wireguard"
    source      = "0.0.0.0/0"
    protocol    = "17"
    udp_options {
      min = 51820
      max = 51820
    }
  }

  lifecycle {
    prevent_destroy = true
  }
}

resource "oci_core_default_route_table" "main" {
  manage_default_resource_id = oci_core_vcn.main.default_route_table_id

  route_rules {
    destination       = "0.0.0.0/0"
    destination_type  = "CIDR_BLOCK"
    network_entity_id = oci_core_internet_gateway.main.id
  }

  lifecycle {
    prevent_destroy = true
  }
}

resource "oci_core_vcn" "main" {
  compartment_id = local.tenancy_ocid
  display_name   = "triss-vcn"
  cidr_blocks    = ["10.0.0.0/16"]
  dns_label      = "vcn04091623"

  lifecycle {
    prevent_destroy = true
  }
}

resource "oci_core_internet_gateway" "main" {
  compartment_id = local.tenancy_ocid
  vcn_id         = oci_core_vcn.main.id
  display_name   = "Internet Gateway triss-vcn"
  enabled        = true

  lifecycle {
    prevent_destroy = true
  }
}

resource "oci_core_subnet" "main" {
  compartment_id    = local.tenancy_ocid
  vcn_id            = oci_core_vcn.main.id
  display_name      = "triss-subnet"
  cidr_block        = "10.0.0.0/24"
  dns_label         = "subnet04091623"
  route_table_id    = oci_core_vcn.main.default_route_table_id
  security_list_ids = [oci_core_vcn.main.default_security_list_id]
  dhcp_options_id   = oci_core_vcn.main.default_dhcp_options_id

  lifecycle {
    prevent_destroy = true
  }
}

resource "oci_core_instance" "triss" {
  compartment_id      = local.tenancy_ocid
  availability_domain = "FJLq:EU-FRANKFURT-1-AD-1"
  fault_domain        = "FAULT-DOMAIN-2"
  display_name        = "triss"
  shape               = "VM.Standard.A1.Flex"

  shape_config {
    ocpus         = 4
    memory_in_gbs = 24
  }

  create_vnic_details {
    subnet_id        = oci_core_subnet.main.id
    assign_public_ip = "true"
    display_name     = "triss-vnic"
    hostname_label   = "triss-vnic"
    private_ip       = "10.0.0.48"
  }

  source_details {
    source_type = "image"
    source_id   = "ocid1.image.oc1.eu-frankfurt-1.aaaaaaaav7j5fmkuvwreezyn7pkyyzgexm4uaobnceclctrmkj2urjvo6e5a"
  }

  metadata = {
    ssh_authorized_keys = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPP7vHu4kMHTI2eK34pUFsXd0ZIPluQvHIh7at0mhviY nikodem@yenn"
  }

  lifecycle {
    prevent_destroy = true
    ignore_changes  = [source_details, metadata]
  }
}
