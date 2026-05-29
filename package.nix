{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  openssl,
  darwin,
}:

let
  srcRoot = ./.;
  srcRootString = toString srcRoot;
  sourceFilter =
    path: type:
    let
      pathString = toString path;
      relPath = lib.removePrefix "${srcRootString}/" pathString;
      baseName = baseNameOf pathString;
      isExcludedTree = dir: relPath == dir || lib.hasPrefix "${dir}/" relPath;
    in
    # Keep the package source closure focused on build inputs: exclude VCS/build
    # outputs plus local coordination state, generated reports, and child
    # worktrees that may live under the repository root during development.
    !(
      baseName == ".git"
      || baseName == "target"
      || baseName == "result"
      || isExcludedTree ".insomnia"
      || isExcludedTree ".worktree"
      || isExcludedTree "work-items"
      || isExcludedTree "docs/report"
    );
in
rustPlatform.buildRustPackage rec {
  pname = "insomnia";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = srcRoot;
    filter = sourceFilter;
  };

  cargoLock.lockFile = ./Cargo.lock;

  strictDeps = true;

  nativeBuildInputs = [ pkg-config ];

  buildInputs = [
    openssl
  ]
  ++ lib.optionals stdenv.hostPlatform.isDarwin (
    with darwin.apple_sdk.frameworks;
    [
      CoreFoundation
      Security
      SystemConfiguration
    ]
  );

  cargoBuildFlags = [
    "-p"
    "pod"
    "-p"
    "tui"
  ];

  # The package check is a credential-free install smoke check below. Running the
  # workspace test suite is intentionally left to cargo-based CI because this
  # derivation is scoped to packaging the user-facing binaries.
  doCheck = false;

  postInstall = ''
    install -Dm644 docs/nix.md "$out/share/doc/insomnia/nix.md"
    mkdir -p "$out/share/insomnia"
    cp -R resources "$out/share/insomnia/resources"
  '';

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck

    "$out/bin/insomnia-pod" --help >/dev/null
    test -x "$out/bin/insomnia"
    if "$out/bin/insomnia" --session not-a-uuid 2>insomnia.err; then
      echo "insomnia unexpectedly accepted an invalid --session value" >&2
      exit 1
    fi
    grep -q "invalid --session UUID" insomnia.err

    test -d "$out/share/insomnia/resources/prompts"
    test -f "$out/share/doc/insomnia/nix.md"

    runHook postInstallCheck
  '';

  meta = {
    description = "Agentic coding Pod runtime and terminal UI";
    license = lib.licenses.mit;
    mainProgram = "insomnia";
    platforms = lib.platforms.unix;
  };
}
