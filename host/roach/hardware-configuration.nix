{
  inputs,
  config,
  pkgs,
  ...
}: {
  imports = [
    inputs.hardware.nixosModules.common-pc-ssd
    (import ../features/disko/btrfs-persistence-swapfile.nix {
      device = "/dev/nvme0n1";
      swap = 6;
    })
    (import ../features/disko/btrfs-single-partition.nix {
      device = "/dev/sda";
    })
  ];

  nixpkgs.hostPlatform.system = "x86_64-linux";
  hardware = {
    enableAllFirmware = true;
    cpu.intel.updateMicrocode = config.hardware.enableRedistributableFirmware;
    firmware = [pkgs.linux-firmware];
  };
  powerManagement.cpuFreqGovernor = "powersave";
  boot = {
    kernelPackages = pkgs.linuxPackages_6_1;
    kernelParams = [
      "pcie_aspm=off"
      "iwlwifi.power_save=0"
    ];
    extraModprobeConfig = ''
      options iwlwifi disable_11ax=1
    '';
    initrd = {
      availableKernelModules = [
        "iwlmvm"
        "iwlwifi"
        "vmd"
        "nvme"
        "xhci_pci"
        "usb_storage"
        "sd_mod"
        "dm-mod"
        "dm-crypt"
        "btrfs"
      ];
      kernelModules = [
        "iwlwifi"
      ];
    };
  };
  security.rtkit.enable = true;
}
