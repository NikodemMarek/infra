{
  persist.users.nikodem = {
    data = {
      directories = [
        "omniverse"
        "games"
      ];
    };
    generated = {
      directories = [
        ".cache"

        ".config/Google"
        ".config/JetBrains"
        ".config/LM-Studio"
        ".config/LM Studio"
        ".lmstudio"

        ".local/share/zoxide"
        ".local/share/direnv"
        ".local/share/keyrings"
        ".local/share/Google"
        ".local/share/claude"
        ".local/share/JetBrains"
        ".local/share/nvim"
        ".local/share/fish"
        ".local/share/PrismLauncher"
        ".local/share/Steam"

        # TODO: For relocation
        ".npm"
        ".cargo"
        ".java"
        ".gradle"
        ".m2"
        ".android"
        ".docker"
        ".dockercache"
        ".mozilla"
        ".factorio"
      ];

      files = [
        ".config/sops/age/keys.txt"

        ".ssh/known_hosts"
      ];
    };
  };
}
