{pkgs, ...}: {
  environment.systemPackages = [
    pkgs.qemu
    pkgs.libvirt
    pkgs.virt-viewer
    pkgs.virt-manager
  ];

  boot = {
    kernelModules = ["kvm-amd"];
    kernelParams = ["amd_iommu=on"];
    initrd.kernelModules = [
      "vfio_pci"
      "vfio"
      "vfio_iommu_type1"
    ];
  };

  virtualisation.libvirtd = {
    enable = true;
    qemu = {
      package = pkgs.qemu_kvm;
      runAsRoot = true;
      swtpm.enable = true;
    };
  };

  networking = {
    firewall.trustedInterfaces = ["virbr0"];
    nftables.ruleset = ''
      table inet libvirt-filter {
        chain input {
          type filter hook input priority -20; policy accept;
          iifname "virbr0" accept comment "Trusted libvirt interfaces"
        }

        chain forward {
          type filter hook forward priority -20; policy accept;
          iifname "virbr0" accept comment "Allow VM traffic to internet"
        }
      }

      table ip libvirt-nat {
        chain postrouting {
          type nat hook postrouting priority srcnat; policy accept;
          ip saddr 192.168.122.0/24 oifname != "virbr0" masquerade
        }
      }
    '';
  };

  persist.generated.directories = [
    {
      directory = "/var/lib/libvirt/";
      user = "root";
      group = "root";
    }
  ];
}
