# Applied as the `additions` overlay (last, see flake.nix) with the final package
# set (it refers to its own attrs) and the previous one as `prev`.
#
# The wrapped packages shadow the nixpkgs ones, so `pkgs.zellij` etc. are the
# wrapped versions. Each wrapper gets its upstream package from `prev`, passed
# explicitly under its own name (callPackage would inject the wrapped one and
# recurse).
{
  pkgs,
  prev,
}: {
  zellij = pkgs.callPackage ./zellij {zellij = prev.zellij;};
  hyprland = pkgs.callPackage ./hyprland {hyprland = prev.hyprland;};
  hyprlock = pkgs.callPackage ./hyprlock {hyprlock = prev.hyprlock;};
  hypridle = pkgs.callPackage ./hypridle {hypridle = prev.hypridle;};
  hyprpaper = pkgs.callPackage ./hyprpaper {hyprpaper = prev.hyprpaper;};
  rofi = pkgs.callPackage ./rofi {rofi = prev.rofi;};
  dunst = pkgs.callPackage ./dunst {dunst = prev.dunst;};
  jujutsu = pkgs.callPackage ./jujutsu {jujutsu = prev.jujutsu;};
  waybar = pkgs.callPackage ./waybar {waybar = prev.waybar;};
  kanshi = pkgs.callPackage ./kanshi {kanshi = prev.kanshi;};
  obsidian = pkgs.callPackage ./obsidian {obsidian = prev.obsidian;};
  zen-browser = pkgs.callPackage ./zen-browser {zen-browser = prev.zen-browser;};
  heroic = pkgs.callPackage ./heroic {heroic = prev.heroic;};
  moonlight-qt = pkgs.callPackage ./moonlight-qt {moonlight-qt = prev.moonlight-qt;};
  prismlauncher = pkgs.callPackage ./prismlauncher {prismlauncher = prev.prismlauncher;};
  glab = pkgs.callPackage ./glab {glab = prev.glab;};
  gh = pkgs.callPackage ./gh {gh = prev.gh;};
  # No nixpkgs `claude` (it wraps `claude-code`). The nulls pin the "use the
  # bundled default" arguments so a same-named top-level package (e.g. `skills`)
  # is never auto-injected by callPackage.
  claude = pkgs.callPackage ./claude {
    instructions = null;
    agents = null;
    skills = null;
    settings = null;
  };

  # Standalone tools. claude takes knowledge, claude-hooks, claude-statusline
  # and jj-upload from the package set, so overrides of them propagate.
  # jj-upload needs the upstream jj: the wrapped one bundles jj-upload (recursion).
  jj-upload = pkgs.callPackage ./jj-upload {jujutsu = prev.jujutsu;};
  jw = pkgs.callPackage ./jw {};
  jail = pkgs.callPackage ./jail {};
  bitwarden-cli = pkgs.callPackage ./bitwarden-cli {bitwarden-cli = prev.bitwarden-cli;};
  wallpaper = pkgs.callPackage ./wallpaper {};
  dictate = pkgs.callPackage ./dictate {};
  agent-skills = pkgs.callPackage ./agent-skills {};
  claude-hooks = pkgs.callPackage ./claude-hooks {};
  claude-statusline = pkgs.callPackage ./claude-statusline {};
  knowledge = pkgs.callPackage ./knowledge {};
  agent-inbox = pkgs.callPackage ./agent-inbox {};

  event-handlers = {
    notify = pkgs.callPackage ./event-handlers/notify {};
    notify-critical = pkgs.event-handlers.notify.override {urgency = "critical";};
    # GitLab event -> Claude session in a shpool session. No gitlab.* routes here (no
    # GitLab watcher): a layer that has one sets contextTool and adds the routes.
    claude = pkgs.callPackage ./event-handlers/claude {
      notify = pkgs.event-handlers.notify;
    };
  };

  # `handlers` stays explicit so `.override (old: {handlers = old.handlers // ...})`
  # can extend it.
  event-router = pkgs.callPackage ./event-router {
    # queued as well as handled; agent-inbox reads the queue
    inbox = ["claude" "sudo"];
    handlers = {
      default = pkgs.event-handlers.notify;
    };
  };
}
