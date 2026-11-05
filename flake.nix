{
  description = "UltraDesk (Rust Edition)";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs = inputs @ {
    self,
    nixpkgs,
    ...
  }: let
    systems = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    forAllSystems = nixpkgs.lib.genAttrs systems;
  in {
    packages = forAllSystems (
      system: let
        pkgs = nixpkgs.legacyPackages.${system};
        ultradesk-rust = pkgs.callPackage ./nix/packages/ultradesk-rust { };
        ultradesk-rust-head = pkgs.callPackage ./nix/packages/ultradesk-rust-head {
          inherit inputs;
        };
      in {
        inherit ultradesk-rust ultradesk-rust-head;
        default = ultradesk-rust;
      }
    );

    nixosModules = {
      ultradesk-rust = import ./nix/modules/nixos/ultradesk-rust {
        inherit inputs;
      };
      default = self.nixosModules.ultradesk-rust;
    };
  };
}
