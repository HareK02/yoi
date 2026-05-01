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
  INSOMNIA_POD_COMMAND = "cargo run -p pod --quiet --";
  shellHook = ''
    echo "dev-shell-loaded"
  '';
}
