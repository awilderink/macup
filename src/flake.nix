{
  description = "The tools and apps listed in macup.toml";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";
    nix-darwin.url = "github:nix-darwin/nix-darwin/nix-darwin-26.05";
    nix-darwin.inputs.nixpkgs.follows = "nixpkgs";
    nix-homebrew.url = "github:zhaofengli/nix-homebrew";
    homebrew-core = { url = "github:homebrew/homebrew-core"; flake = false; };
    homebrew-cask = { url = "github:homebrew/homebrew-cask"; flake = false; };
  };

  outputs = { nix-darwin, nix-homebrew, homebrew-core, homebrew-cask, ... }:
    let
      mac = builtins.fromJSON (builtins.readFile ./mac.json);
    in
    {
      darwinConfigurations.macup = nix-darwin.lib.darwinSystem {
        modules = [
          nix-homebrew.darwinModules.nix-homebrew
          ({ config, lib, pkgs, ... }: {
            environment.systemPackages = map (name: lib.getAttrFromPath (lib.splitString "." name) pkgs) mac.nix;
            homebrew.brews = mac.brew;
            homebrew.casks = mac.cask;

            system.primaryUser = mac.user;
            system.stateVersion = 6;
            nixpkgs.hostPlatform = "aarch64-darwin";
            nixpkgs.config.allowUnfree = true;

            nix.package = pkgs.lix;
            nix.settings.experimental-features = [ "nix-command" "flakes" ];
            nix.gc.automatic = true;
            nix.gc.options = "--delete-older-than 30d";
            nix.optimise.automatic = true;
            programs.zsh.enable = true;

            nix-homebrew = {
              enable = true;
              user = config.system.primaryUser;
              autoMigrate = true;
              taps = {
                "homebrew/homebrew-core" = homebrew-core;
                "homebrew/homebrew-cask" = homebrew-cask;
              };
              mutableTaps = false;
            };

            homebrew = {
              enable = true;
              taps = builtins.attrNames config.nix-homebrew.taps;
              onActivation = {
                autoUpdate = false;
                upgrade = false;
                cleanup = "none";
              };
            };

            system.activationScripts.postActivation.text = lib.optionalString (builtins.elem "rustup" mac.nix) ''
              if ! /usr/bin/sudo --user=${config.system.primaryUser} --set-home ${pkgs.rustup}/bin/rustup default stable; then
                echo "rustup: could not install the stable toolchain; rerun the apply or 'rustup default stable'" >&2
              fi
            '';
          })
        ];
      };
    };
}
