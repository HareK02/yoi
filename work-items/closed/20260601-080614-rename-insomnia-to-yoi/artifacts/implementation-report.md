# Implementation report: Yoi rename

Implemented a clean public rename from Insomnia to Yoi.

## Scope completed

- Renamed the main Cargo package/crate/binary from `insomnia` to `yoi`:
  - `crates/insomnia/` -> `crates/yoi/`
  - workspace package/dependency metadata updated
  - CLI help and tests now use `yoi`
- Updated Nix packaging:
  - package `pname` is `yoi`
  - flake package/app output points at `bin/yoi`
  - `cargoHash` refreshed after the lock/vendor input changed
- Updated CLI/runtime command surfaces:
  - `insomnia pod` -> `yoi pod`
  - `INSOMNIA_POD_RUNTIME_COMMAND` -> `YOI_POD_RUNTIME_COMMAND`
  - `INSOMNIA-READY` -> `YOI-READY`
  - runtime fallback command now targets `yoi`
- Updated path/env authority:
  - `.insomnia` -> `.yoi`
  - `~/.insomnia` -> `~/.yoi`
  - config/runtime/data env prefixes use `YOI_*`
- Updated prompt/profile namespace:
  - `$insomnia` -> `$yoi`
  - `require("insomnia")` -> `require("yoi")`
  - Lua profile API/version strings updated to Yoi naming
- Updated active docs, prompt resources, tests, diagnostics, examples, and help strings.
- Kept old-name references only where they are rationale or historical records, such as `docs/branding.md`, historical reports, and work-item history.

## Compatibility decision

No old command aliases, old environment-variable aliases, old prompt/profile aliases, or old search roots are implemented. Yoi paths are the active paths.

## Residual old-name references in active surfaces

The active non-historical residual grep should not include old product/runtime identifiers. Remaining old-name references are expected only in rationale/historical records.

## Validation

Passed before the correction to remove compatibility remnants:

- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p manifest`
- `cargo test -p yoi`
- `cargo test -p client runtime_command`
- `cargo test -p pod entrypoint`
- `cargo test -p pod prompt::loader`
- `cargo test -p manifest profile`
- `target/debug/yoi --help`
- `target/debug/yoi pod --help`
- `nix eval .#packages.x86_64-linux.default.pname --raw`
- `nix eval .#apps.x86_64-linux.default.program --raw`
- `nix build .#yoi --no-link`
- `git diff --check`

Additional validation after removing compatibility remnants:

- `cargo fmt --check`
- `cargo test -p manifest paths`
- `cargo test -p pod spawn_profile_selector_accepts_default_inherit_and_registry`

Run the final full validation set again before closing this work item.
