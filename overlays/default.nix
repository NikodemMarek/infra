{inputs, ...}: {
  # Shadows nixpkgs packages with wrapped ones: apply it after the overlays that
  # set their upstream packages (inputs-packages: hyprland), see flake.nix.
  additions = final: prev:
    import ../pkgs {
      pkgs = final;
      inherit prev;
    };

  modifications = final: prev: {};

  unstable-packages = final: _prev: {
    unstable = import inputs.nixpkgs {
      system = final.stdenv.hostPlatform.system;
      config.allowUnfree = true;
      config.allowUnfreePredicate = _: true;
    };
  };

  inputs-packages = final: prev: {
    inherit (inputs.hyprland.packages.${prev.stdenv.hostPlatform.system}) hyprland xdg-desktop-portal-hyprland;
    neovim = inputs.neovim.packages.${prev.stdenv.hostPlatform.system}.default;
    inherit (inputs.zen-browser.packages.${prev.stdenv.hostPlatform.system}) zen-browser;
    inherit (inputs.outlay.packages.${prev.stdenv.hostPlatform.system}) outlay;
  };

  deploy-rs = final: prev: {
    deploy-rs = {
      inherit (prev) deploy-rs;
      inherit ((inputs.deploy-rs.overlays.default final prev).deploy-rs) lib;
    };
  };
}
