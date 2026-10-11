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

        ".config/LM-Studio"
        ".config/LM Studio"
        ".lmstudio"

        ".local/share/zoxide"
        ".local/share/direnv"
        ".local/share/claude"
        ".local/share/nvim"
        ".local/share/fish"
        ".local/share/PrismLauncher"
        ".local/share/Steam"

        # Hardcoded paths, can't be relocated
        ".java"
        ".factorio"
      ];

      files = [
        ".config/sops/age/keys.txt"

        ".ssh/known_hosts"
      ];
    };
  };
}
