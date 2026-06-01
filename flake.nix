{
  description = "Yoi agent";

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
        yoi = pkgs.callPackage ./package.nix { };
        mkApp = name: description: {
          type = "app";
          program = "${yoi}/bin/${name}";
          meta.description = description;
        };
      in
      {
        packages.default = yoi;
        packages.yoi = yoi;

        apps.default = mkApp "yoi" "Run the Yoi terminal UI";
        apps.yoi = mkApp "yoi" "Run the Yoi terminal UI";

        checks.default = yoi;

        devShells.default = import ./devshell.nix { inherit pkgs; };
      }
    );
}
