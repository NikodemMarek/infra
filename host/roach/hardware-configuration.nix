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
    printers = {
      ensureDefaultPrinter = "Canon_MP280";
      ensurePrinters = [
        {
          name = "Canon_MP280";
          location = "Home";
          deviceUri = "usb://Canon/MP280%20series?serial=698AFF&interface=1";
          model = "gutenprint.5.3://bjc-MULTIPASS-MP280/expert";
          ppdOptions = {
            PageSize = "A4";
          };
        }
      ];
    };
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
