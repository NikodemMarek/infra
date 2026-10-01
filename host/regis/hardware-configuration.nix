{
  inputs,
  pkgs,
  config,
  ...
}: let
  # Package the Realtek r8126 module against the active kernel
  r8126-module =
    config.boot.kernelPackages.callPackage
    ./realtek-r8126.nix
    {};
in {
  imports = [
    inputs.hardware.nixosModules.common-pc-ssd

    ./tpm2.nix
    ./storage.nix
  ];

  nixpkgs.hostPlatform.system = "x86_64-linux";

  networking.hostId = "83745928";

  hardware.enableRedistributableFirmware = true;

  boot = {
    supportedFilesystems = ["zfs"];
    kernelPackages = pkgs.linuxPackages_6_18;
    blacklistedKernelModules = ["r8169"];
    extraModulePackages = [r8126-module];
    initrd = {
      availableKernelModules = [
        "zfs"
        "nvme"
        "xhci_pci"
        "ahci"
        "usb_storage"
        "sd_mod"
        "dm_crypt"
      ];
      kernelModules = ["kvm-intel" "zfs" "r8126"];
    };
    zfs = {
      forceImportRoot = false;
      requestEncryptionCredentials = true;
      extraPools = ["tank"];
    };
  };

  services.zfs = {
    autoScrub.enable = true;
    trim.enable = true;
  };
}
