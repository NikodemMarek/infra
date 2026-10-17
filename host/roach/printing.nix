{pkgs, ...}: {
  services = {
    avahi = {
      enable = true;

      publish = {
        enable = true;
        userServices = true;
      };
    };
    printing = {
      enable = true;
      listenAddresses = ["*:631"];
      allowFrom = ["all"];
      browsing = true;
    };
    samba = {
      enable = true;

      settings = {
        "global" = {
          "load printers" = "yes";
          "printing" = "cups";
          "printcap name" = "cups";
        };
        "printers" = {
          "comment" = "All Printers";
          "path" = "/var/spool/samba";
          "public" = "yes";
          "browseable" = "yes";
          "guest ok" = "yes";
          "writable" = "no";
          "printable" = "yes";
          "create mode" = 0700;
        };
      };
    };
  };
  systemd.tmpfiles.rules = [
    "d /var/spool/samba 1777 root root -"
  ];

  networking.nftables.tables = {
    printing-filter = {
      family = "inet";
      content = builtins.readFile ./printing-filter.nft;
    };
  };
}
