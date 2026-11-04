{
  imports = [
    ./networking.nix
    ./nix.nix
    ./openssh.nix
    ./sops.nix
  ];

  programs.git.enable = true;
  environment.variables.EDITOR = "nvim";
  security.sudo.enable = true;
}
