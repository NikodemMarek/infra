{
  config,
  lib,
  modulesPath,
  ...
}: {
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")

    (import ../features/disko/btrfs-persistence-swapfile.nix {
      device = "/dev/nvme0n1";
      swap = 38;
    })
  ];

  boot = {
    initrd.availableKernelModules = ["nvme" "xhci_pci" "ahci" "usbhid"];
    initrd.kernelModules = [];
    kernelModules = [
      "kvm-amd"
      "vfio_pci"
      "vfio"
      "vfio_iommu_type1"
    ];
    kernelParams = [];
    extraModprobeConfig = "";
    extraModulePackages = [];
  };

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}
