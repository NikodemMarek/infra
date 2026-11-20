{config, ...}: {
  # The hash must exist before users are created, otherwise the deploy user
  # has no password and sudo breaks
  secrets.users_maintenance_password.neededForUsers = true;

  nix.settings.trusted-users = ["maintenance" "@wheel"];

  security.sudo.extraRules = [
    {
      users = ["maintenance"];
      runAs = "root";
      commands = [
        {
          command = "/nix/store/*/activate-rs";
          options = ["NOPASSWD"];
        }
        {
          command = "/run/current-system/sw/bin/rm /tmp/deploy-rs-canary-*";
          options = ["NOPASSWD"];
        }
      ];
    }
  ];

  users = {
    users = {
      root.hashedPassword = "!";
      maintenance = {
        isNormalUser = true;
        hashedPasswordFile = config.secrets.users_maintenance_password.path;
        extraGroups = ["wheel" "docker"];
        openssh.authorizedKeys = {
          keyFiles = [
            ../../yenn/user_nikodem_ssh_id_ed25519.pub
          ];
          keys = [
            ''restrict,from="100.64.0.0/10,fd7a:115c:a1e0::/48" ${builtins.readFile ./ci_ssh_id_ed25519.pub}''
          ];
        };
      };
    };
  };
}
