_: {
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

  networking.nftables.ruleset = ''
    table inet printing-filter {
      chain input {
        type filter hook input priority -30; policy accept;

        # Avahi (UDP 5353)
        udp dport 5353 accept comment "Avahi mDNS"

        # CUPS (TCP/UDP 631)
        tcp dport 631 accept comment "CUPS IPP"
        udp dport 631 accept comment "CUPS IPP"

        # Samba (UDP 137, 138; TCP 139, 445; UDP 445)
        udp dport { 137, 138 } accept comment "Samba NetBIOS"
        tcp dport { 139, 445 } accept comment "Samba SMB/NetBIOS"
        udp dport 445 accept comment "Samba SMB/NetBIOS"
      }
    }
  '';
}
