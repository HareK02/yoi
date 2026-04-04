{ pkgs }:
pkgs.mkShell {
  packages = with pkgs; [
    nixfmt
    deno
    git
    rustc
    cargo
  ];
  buildInputs = with pkgs; [
    pkg-config
    openssl
  ];
  shellHook = ''
    echo "dev-shell-loaded"
  '';
}
