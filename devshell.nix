{ pkgs }:
pkgs.mkShell {
  buildInputs = with pkgs; [
    rustc
    cargo
  ];
  shellHook = ''
    echo "dev-shell-loaded"
  '';
}
