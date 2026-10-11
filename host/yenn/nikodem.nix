{pkgs, ...}: let
  projects = "/persist/data/home/nikodem/projects";
  bitwarden = "/persist/generated/home/nikodem/bitwarden";
in {
  imports = [
    ../features/optional/syncthing.nix
    ../features/optional/docker.nix
    ./persist.nix
  ];

  environment = {
    systemPackages = [
      pkgs.kanshi
      pkgs.antigravity-cli
      pkgs.signal-desktop
      pkgs.jujutsu
      pkgs.zellij

      pkgs.claude
      pkgs.agent-inbox
      pkgs.jj-upload
      (pkgs.jw.override {projectsRoot = projects;})
      (pkgs.bitwarden-cli.override {dataDir = bitwarden;})
      pkgs.git

      pkgs.alacritty
      pkgs.remmina
      pkgs.google-chrome
      pkgs.zen-browser
      pkgs.obsidian
      pkgs.ripgrep
      pkgs.eza
      pkgs.jq
      pkgs.zip
      pkgs.unzip
      pkgs.bottom
      pkgs.xxd
      pkgs.fd
      pkgs.bat
      pkgs.feh
      pkgs.mpv
      pkgs.tldr
      pkgs.ffmpeg
      pkgs.openssl
      pkgs.rnote
      pkgs.zathura
      pkgs.kooha
      pkgs.impala
      pkgs.bluetui
      pkgs.wiremix
      pkgs.yazi
      pkgs.nmap
      pkgs.tcpdump
      pkgs.lsof
      pkgs.devenv
      pkgs.neovim
      pkgs.vscode-extensions.vadimcn.vscode-lldb.adapter
      pkgs.opencode
      pkgs.libreoffice
      pkgs.lmstudio

      pkgs.prismlauncher
      pkgs.heroic
      pkgs.steam
      pkgs.lutris
      pkgs.wine
      pkgs.winetricks
    ];
    shellAliases = {
      l = "eza -la --icons --group-directories-first --git";
      lt = "eza -laT --icons --group-directories-first --git";
      n = "nvim";
      zj = "zellij";
    };
  };

  programs = {
    fish = {
      enable = true;
      interactiveShellInit = ''
        # Fully clear console.
        function fish_user_key_bindings
          bind \cl 'clear; commandline -f repaint'
        end

        function fish_greeting
        end

        fish_vi_key_bindings
      '';
    };
    direnv = {
      enable = true;
      enableBashIntegration = true;
      enableFishIntegration = true;
      nix-direnv.enable = true;
      settings.whitelist.prefix = [projects];
    };
    zoxide = {
      enable = true;
      enableFishIntegration = true;
      enableBashIntegration = true;
    };
    starship = {
      enable = true;
      settings = builtins.fromTOML (builtins.readFile ./starship.toml);
    };
    hyprland = {
      enable = true;
      withUWSM = true;
      portalPackage = pkgs.xdg-desktop-portal-hyprland;
      xwayland.enable = true;
    };
    hyprlock.enable = true;
  };

  secrets.users_nikodem_ssh_id_ed25519 = {
    owner = "nikodem";
    group = "users";
    path = "/home/nikodem/.ssh/id_ed25519";
  };
  systemd.tmpfiles.rules = [
    "d ${projects} 0755 nikodem users -"
    "d ${bitwarden} 0700 nikodem users -"
    "d /home/nikodem/.ssh 0700 nikodem users -"
    "L+ /home/nikodem/.ssh/id_ed25519.pub 0400 nikodem users - ${./user_nikodem_ssh_id_ed25519.pub}"
  ];

  services = {
    event-router.enable = true;
    hypridle.enable = true;
    syncthing.user = "nikodem";
  };
}
