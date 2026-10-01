# Unlock rpool from the TPM2 at boot; falls back to the passphrase prompt.
# One-time setup, run on regis from the repo root in a root shell:
#   systemd-ask-password -n "rpool passphrase:" | systemd-creds encrypt --with-key=tpm2 --tpm2-pcrs=7 --name=rpool - host/regis/rpool.cred
# Re-run it if the TPM is cleared or PCR 7 (Secure Boot state) changes.
{config, ...}: {
  boot.initrd.systemd = {
    contents."/etc/zfs/rpool.cred".source = ./rpool.cred;
    services.zfs-unlock-rpool-tpm2 = {
      description = "Unlock rpool with the TPM2-sealed passphrase";
      unitConfig.DefaultDependencies = false;
      wantedBy = ["zfs-import-rpool.service"];
      before = ["zfs-import-rpool.service"];
      wants = [config.persist.deviceService "tpm2.target" "systemd-modules-load.service"];
      after = [config.persist.deviceService "tpm2.target" "systemd-modules-load.service"];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # A hang must not hold back the passphrase prompt in zfs-import-rpool.
        TimeoutStartSec = "30s";
      };
      # Must never fail: zfs-import-rpool prompts for the passphrase if the key is still locked.
      script = ''
        zpool import -d ${config.boot.zfs.devNodes} -N rpool || exit 0
        systemd-creds decrypt --name=rpool /etc/zfs/rpool.cred - | zfs load-key rpool \
          || echo "TPM2 unlock of rpool failed, falling back to passphrase prompt"
      '';
    };
  };
}
