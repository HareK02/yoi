{ pkgs }:
pkgs.mkShell {
  packages = with pkgs; [
    nixfmt
    deno
    git
    playwright-driver.browsers
    rustc
    cargo
    pkgs.sccache
  ];

  PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers}";
  PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";

  # Cache storage and limits belong to the host's sccache configuration.
  RUSTC_WRAPPER = "${pkgs.sccache}/bin/sccache";

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
