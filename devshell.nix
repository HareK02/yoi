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
    if repo_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
      : # export YOI_POD_RUNTIME_COMMAND="$repo_root/target/debug/yoi"
    else
      : # export YOI_POD_RUNTIME_COMMAND="$PWD/target/debug/yoi"
    fi
    echo "dev-shell-loaded"
    echo "YOI_POD_RUNTIME_COMMAND=$YOI_POD_RUNTIME_COMMAND"
  '';
}
