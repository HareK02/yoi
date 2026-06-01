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
      || isExcludedTree ".yoi"
      || isExcludedTree ".worktree"
      || isExcludedTree "work-items"
      || isExcludedTree "docs/report"
    );
in
rustPlatform.buildRustPackage rec {
  pname = "yoi";
  version = "0.1.0";

  src = lib.cleanSourceWith {
    src = srcRoot;
    filter = sourceFilter;
  };

  cargoHash = "sha256-f4/oOuPv4dUiwznX+popMjjDCXZQPBvqWRYmlJDyKkE=";

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
    "yoi"
  ];

  # The package check is a credential-free install smoke check below. Running the
  # workspace test suite is intentionally left to cargo-based CI because this
  # derivation is scoped to packaging the user-facing binaries.
  doCheck = false;

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck

    "$out/bin/yoi" pod --help >/dev/null
    test -x "$out/bin/yoi"
    test ! -e "$out/bin/yoi-pod"
    test ! -e "$out/share/yoi/resources"
    if "$out/bin/yoi" --session not-a-uuid 2>yoi.err; then
      echo "yoi unexpectedly accepted an invalid --session value" >&2
      exit 1
    fi
    grep -q "invalid --session UUID" yoi.err

    runHook postInstallCheck
  '';

  meta = {
    description = "Agentic coding Pod runtime and terminal UI";
    license = lib.licenses.mit;
    mainProgram = "yoi";
    platforms = lib.platforms.unix;
  };
}
