{
  description = "system configuration";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";

    systems.url = "github:nix-systems/default-linux";

    hardware = {
      url = "github:nixos/nixos-hardware";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    sops-nix = {
      url = "github:Mic92/sops-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # Overridden at build time with `--override-input` (host-switch, host-deploy,
    # install-remote) by the directory `secrets-build` writes, see devenv.nix
    secrets = {
      url = "path:./secrets-stub";
      flake = false;
    };

    disko = {
      url = "github:nix-community/disko";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    impermanence = {
      url = "github:nix-community/impermanence";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    deploy-rs = {
      url = "github:serokell/deploy-rs";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    stylix = {
      url = "github:danth/stylix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    hyprland = {
      url = "github:hyprwm/Hyprland";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    neovim.url = "github:NikodemMarek/neovim";

    zen-browser = {
      url = "github:youwen5/zen-browser-flake";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    outlay = {
      url = "github:Papayah/outlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    systems,
    ...
  } @ inputs: let
    inherit (self) outputs;

    lib = nixpkgs.lib;

    allOverlays = import ./overlays {inherit inputs;};

    forEachSystem = f: lib.genAttrs (import systems) (system: f pkgsFor.${system});
    pkgsFor = lib.genAttrs (import systems) (system:
      import nixpkgs {
        inherit system;
        config.allowUnfree = true;
        # additions last: its wrappers shadow (and wrap) the packages set by the others
        overlays = lib.attrValues (removeAttrs allOverlays ["additions"]) ++ [allOverlays.additions];
      });
  in {
    inherit lib;

    nixosModules = import ./modules/host;

    overlays = allOverlays;
    # everything the additions overlay adds; flake packages must be derivations, so
    # the jail function is left out and the event-handlers set is flattened
    packages = forEachSystem (pkgs:
      removeAttrs (builtins.intersectAttrs (allOverlays.additions pkgs pkgs) pkgs) ["jail" "event-handlers"]
      // lib.mapAttrs' (name: lib.nameValuePair "event-handler-${name}") pkgs.event-handlers);

    nixosConfigurations = let
      mkHost = host: system:
        lib.nixosSystem {
          pkgs = pkgsFor.${system};
          specialArgs = {inherit inputs outputs;};
          modules = [./host/${host}];
        };
    in {
      yenn = mkHost "yenn" "x86_64-linux";
      geralt = mkHost "geralt" "x86_64-linux";
      roach = mkHost "roach" "x86_64-linux";
      regis = mkHost "regis" "x86_64-linux";
      triss = mkHost "triss" "aarch64-linux";
    };

    deploy.nodes = let
      mkNode = host: extra: let
        cfg = self.nixosConfigurations.${host};
      in
        {
          hostname = host;
          profiles.system = {
            user = "root";
            sshUser = "maintenance";
            interactiveSudo = true;
            path = cfg.pkgs.deploy-rs.lib.activate.nixos cfg;
          };
        }
        // extra;
    in {
      geralt = mkNode "geralt" {};
      roach = mkNode "roach" {};
      regis = mkNode "regis" {};
      triss = mkNode "triss" {};
    };

    checks =
      forEachSystem (pkgs:
        pkgs.deploy-rs.lib.deployChecks self.deploy);
  };
}
