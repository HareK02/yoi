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
        dockerImages =
          if pkgs.stdenv.isLinux then
            import ./docker.nix {
              inherit pkgs yoi;
            }
          else
            { };
        mkApp = name: description: {
          type = "app";
          program = "${yoi}/bin/${name}";
          meta.description = description;
        };
      in
      {
        packages = {
          default = yoi;
          yoi = yoi;
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          docker-runtime = dockerImages.runtime;
          docker-server = dockerImages.server;
          docker-webui = dockerImages.webui;
          webui-static = dockerImages.webui-static;
        };

        apps.default = mkApp "yoi" "Run the Yoi terminal UI";
        apps.yoi = mkApp "yoi" "Run the Yoi terminal UI";

        checks.default = yoi;

        devShells.default = import ./devshell.nix { inherit pkgs; };
      }
    );
}
