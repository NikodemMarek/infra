{...}: {
  boot = {
    kernelModules = ["iTCO_wdt"];
    blacklistedKernelModules = ["intel_oc_wdt"];
    kernelParams = ["panic=10" "zfs.zfs_arc_max=1073741824"];
    kernel.sysctl = {
      "kernel.panic" = 10;
      "kernel.panic_on_oops" = 1;
      "kernel.softlockup_panic" = 1;
      "kernel.hardlockup_panic" = 1;
    };
  };

  systemd.settings.Manager.RuntimeWatchdogSec = "30s";

  services.earlyoom = {
    enable = true;
    freeMemThreshold = 4;
    freeMemKillThreshold = 2;
    extraArgs = ["--avoid" "^(k3s|k3s-agent|k3s-server|containerd|containerd-shim|garage|sshd|sshd-session|tailscaled|systemd|systemd-.*)$"];
  };

  services.k3s.extraFlags = [
    "--kubelet-arg=system-reserved=memory=2Gi"
    "--kubelet-arg=eviction-hard=memory.available<500Mi,nodefs.available<5%%,imagefs.available<5%%"
  ];
}
