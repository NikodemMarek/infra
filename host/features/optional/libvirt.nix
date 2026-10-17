{pkgs, ...}: {
  environment.systemPackages = [
    pkgs.qemu
    pkgs.libvirt
    pkgs.virt-viewer
    pkgs.virt-manager
  ];

  boot.kernelModules = ["kvm-amd"];
  boot.kernelParams = ["amd_iommu=on"];
  virtualisation.libvirtd = {
    enable = true;
    qemu = {
      package = pkgs.qemu_kvm;
      runAsRoot = true;
      swtpm.enable = true;
    };
  };

  boot.initrd.kernelModules = [
    "vfio_pci"
    "vfio"
    "vfio_iommu_type1"
  ];

  networking = {
    firewall.trustedInterfaces = ["virbr0"];
    nftables.tables = {
      libvirt-filter = {
        family = "inet";
        content = builtins.readFile ./libvirt-filter.nft;
      };
      libvirt-nat = {
        family = "ip";
        content = builtins.readFile ./libvirt-nat.nft;
      };
    };
  };

  persist.generated.directories = [
    {
      directory = "/var/lib/libvirt/";
      user = "root";
      group = "root";
    }
  ];
}
