{
  description = "pleamar-wm: a Wayland compositor whose window manager is a pleamar scene";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    pleamar = {
      url = "github:k4ditano/pleamar/no-layouts";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    marea = {
      url = "github:k4ditano/marea-plm";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.pleamar.follows = "pleamar";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      pleamar,
      marea,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (
        system: pkgs: rec {
          pleamar-wm = pkgs.callPackage ./nix/package.nix { pleamarSrc = pleamar.outPath; };
          default = pleamar-wm;
        }
      );

      # `nix run github:k4ditano/pleamar-wm -- headless` (or `session` from a TTY)
      apps = forAll (
        system: _pkgs: {
          default = {
            type = "app";
            program = "${self.packages.${system}.pleamar-wm}/bin/pleamar-wm";
          };
        }
      );

      # NixOS: `programs.pleamar-wm.enable = true;`
      nixosModules.default = import ./nix/nixos.nix self;
    };
}
