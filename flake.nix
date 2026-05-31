{
  description = "INSOMNIA agent runtime";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        insomnia = pkgs.callPackage ./package.nix { };
        mkApp = name: description: {
          type = "app";
          program = "${insomnia}/bin/${name}";
          meta.description = description;
        };
      in
      {
        packages.default = insomnia;
        packages.insomnia = insomnia;

        apps.default = mkApp "insomnia" "Run the INSOMNIA terminal UI";
        apps.insomnia = mkApp "insomnia" "Run the INSOMNIA terminal UI";

        checks.default = insomnia;

        devShells.default = import ./devshell.nix { inherit pkgs; };
      }
    );
}
