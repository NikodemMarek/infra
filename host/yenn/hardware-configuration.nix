{
  inputs,
  pkgs,
  ...
}: {
  imports = [
    inputs.hardware.nixosModules.common-pc-ssd
    inputs.hardware.nixosModules.framework-amd-ai-300-series

    (import ../features/disko/luks-btrfs-persistence-swapfile.nix {
      device = "/dev/nvme0n1";
      swap = 24;
    })
  ];

  nixpkgs.hostPlatform.system = "x86_64-linux";
  hardware = {
    cpu.amd.updateMicrocode = true;
    graphics.enable = true;
    enableAllFirmware = true;
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
  powerManagement.cpuFreqGovernor = "ondemand";
  boot = {
    kernelPackages = pkgs.linuxPackages_latest;
    initrd = {
      availableKernelModules = [
        "nvme"
        "btmtk"
        "xhci_pci"
        "thunderbolt"
        "usb_storage"
        "sd_mod"
      ];
      kernelModules = ["kvm-amd"];
    };
  };
  security.rtkit.enable = true;
}
