{
  pkgs,
  config,
  ...
}: {
  imports = [
    ./hardware-configuration.nix
    ./networking.nix
    ./nikodem.nix

    ../features
    ../features/optional/stylix.nix
    ../features/optional/systemd-boot.nix
    ../features/optional/bluetooth.nix
    ../features/optional/pipewire.nix
    ../features/optional/libvirt.nix
    ../features/optional/dictate.nix
  ];

  networking.hostName = "yenn";
  time.timeZone = "Europe/Warsaw";

  boot.binfmt.emulatedSystems = ["aarch64-linux"];

  persist = {
    enable = true;
    deviceService = "dev-nvme0n1p2.device";
    rootPath = "/dev/mapper/crypted";
    isCrypted = true;
  };

  nix.settings.trusted-users = ["root" "nikodem"];
  secrets.users_nikodem_password.neededForUsers = true;

  users.users = {
    nikodem = {
      isNormalUser = true;
      hashedPasswordFile = config.secrets.users_nikodem_password.path;
      extraGroups = ["wheel" "docker" "libvirtd"];
      shell = pkgs.fish;
      openssh.authorizedKeys.keyFiles = [
        ./user_nikodem_ssh_id_ed25519.pub
      ];
    };
  };

  services = {
    greetd = {
      enable = true;
      useTextGreeter = true;
      settings = {
        default_session = {
          command = "${pkgs.tuigreet}/bin/tuigreet -r -t --cmd 'uwsm start Hyprland'";
        };
      };
    };
    printing = {
      enable = true;
      drivers = [pkgs.gutenprint];
    };
    battery-notifier = {
      enable = true;
      capacityPath = "/sys/class/power_supply/BAT1/capacity";
      statusPath = "/sys/class/power_supply/BAT1/status";
    };
  };
}
