locals {
  tenancy_ocid = "ocid1.tenancy.oc1..aaaaaaaa2vaiux5lkc54bkn46n3zjekb536aqv3i66sghoii6uaumvwxl3bq"
}

provider "oci" {
  tenancy_ocid = local.tenancy_ocid
  user_ocid    = "ocid1.user.oc1..aaaaaaaak2wx72vcsjzmylydbig6cmtkanivs3bzzsytra35r7bkiakqfjuq"
  fingerprint  = "36:84:1b:63:33:d3:be:31:21:cc:18:b9:0e:a6:ed:3e"
  region       = "eu-frankfurt-1"
}
