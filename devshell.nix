{ pkgs }:
let
  # Dev-only wrapper. tui の spawn 経路は `insomnia-pod` バイナリを直に exec し、
  # stderr の `INSOMNIA-READY` 行で握手するので、cargo の進捗や rustc の
  # warning が混ざると tail に余計な行が積もり本当のエラーが押し出される。
  # ここで一度ビルドを切り離し、成功時はビルド出力を一切捨てて素のバイナリ
  # を exec、失敗時のみ build log を stderr に流して exit する。
  pod-dev = pkgs.writeShellScriptBin "insomnia-pod" ''
    set -u
    buildlog=$(mktemp)
    trap 'rm -f "$buildlog"' EXIT
    if ! cargo build --quiet -p pod 2>"$buildlog"; then
      cat "$buildlog" >&2
      exit 1
    fi
    manifest=$(cargo locate-project --workspace --message-format plain 2>/dev/null)
    target_dir=''${CARGO_TARGET_DIR:-$(dirname "$manifest")/target}
    exec "$target_dir/debug/insomnia-pod" "$@"
  '';
in
pkgs.mkShell {
  packages = with pkgs; [
    nixfmt
    deno
    git
    rustc
    cargo
    pod-dev
  ];
  buildInputs = with pkgs; [
    pkg-config
    openssl
  ];
  shellHook = ''
    echo "dev-shell-loaded"
  '';
}
