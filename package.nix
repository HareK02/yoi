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

  cargoHash = "sha256-VzVFqOWJHfgX92Qw84995ICQu2uvQPeYm6AotU4/LR0=";

  depsExtraArgs = {
    # nixpkgs 25.11's fetchCargoVendor still uses crates.io's API
    # download endpoint in this environment, which returns 403 while the
    # immutable static CDN endpoint works. Keep this local package build on
    # static.crates.io until the upstream fetcher is fixed in our nixpkgs pin.
    buildPhase = ''
      runHook preBuild

      if [ -n "''${cargoRoot-}" ]; then
        cd "$cargoRoot"
      fi

      vendor_util="$(command -v fetch-cargo-vendor-util-v2 || command -v fetch-cargo-vendor-util)"
      cp "$vendor_util" ./fetch-cargo-vendor-util-static
      substituteInPlace ./fetch-cargo-vendor-util-static \
        --replace-fail 'https://crates.io/api/v1/crates/{pkg["name"]}/{pkg["version"]}/download' \
                       'https://static.crates.io/crates/{pkg["name"]}/{pkg["version"]}/download'
      ./fetch-cargo-vendor-util-static create-vendor-staging ./Cargo.lock "$out"

      runHook postBuild
    '';
  };

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
