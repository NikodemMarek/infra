{modulesPath, ...}: {
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")

    (import ../features/disko/btrfs-persistence-swapfile.nix {
      device = "/dev/sda";
      swap = 12;
    })
  ];

  time.timeZone = "Etc/UTC";
  boot = {
    loader.efi.canTouchEfiVariables = true;
    initrd = {
      availableKernelModules = ["xhci_pci" "virtio_pci" "virtio_scsi" "usbhid"];
      kernelModules = ["kvm" "br_netfilter" "overlay" "veth" "vxlan"];
    };
  };
  security.rtkit.enable = true;
  nixpkgs.hostPlatform.system = "aarch64-linux";
}
