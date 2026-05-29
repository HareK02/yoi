# Nix package

INSOMNIA provides a flake package for installing the user-facing Pod CLI and TUI binaries without relying on a source checkout at runtime.

## Build

From the repository root:

```sh
nix build .#
```

The default package is implemented by `package.nix` and builds the Cargo packages `pod` and `tui` as installed binaries `insomnia-pod` and `insomnia`. The derivation uses the checked-in `Cargo.lock`, so Cargo dependencies are fetched by the normal Nix Rust packaging path instead of by network access during the build.

The package output contains:

- `bin/insomnia-pod` — Pod CLI / runtime process.
- `bin/insomnia` — terminal UI.
- `share/insomnia/resources/` — bundled runtime resources, including `resources/prompts/`.
- `share/doc/insomnia/nix.md` — this document.

## Run

After `nix build`:

```sh
./result/bin/insomnia-pod --help
./result/bin/insomnia
```

With flakes:

```sh
nix run .#insomnia
nix run .#insomnia-pod -- --help
```

`nix run .#` defaults to the TUI.

## Configuration discovery

The Nix package does not put user configuration, sessions, sockets, or other mutable state in the Nix store. The installed binaries keep the same path semantics as non-Nix builds:

| Purpose | Override | `INSOMNIA_HOME` fallback | XDG / default fallback |
| --- | --- | --- | --- |
| User config (`profiles.toml`, prompt overrides, model/provider overrides) | `INSOMNIA_CONFIG_DIR` | `$INSOMNIA_HOME/config` | `$XDG_CONFIG_HOME/insomnia`, then `$HOME/.config/insomnia` |
| Persistent data (`sessions/`, Pod metadata) | `INSOMNIA_DATA_DIR` | `$INSOMNIA_HOME` | `$HOME/.insomnia` |
| Runtime state (sockets, lock files, live registry) | `INSOMNIA_RUNTIME_DIR` | `$INSOMNIA_HOME/run` | `$XDG_RUNTIME_DIR/insomnia`, then `$HOME/.insomnia/run` |

Normal fresh startup is profile-based. The package ships a builtin default profile, user/project `profiles.toml` files may select or define profiles, and `insomnia-pod --manifest <PATH>` remains a one-file compatibility/debug input. `INSOMNIA_USER_MANIFEST` and ambient `.insomnia/manifest.toml` discovery are not part of normal Pod/TUI startup.

## Validation

The package derivation has a credential-free install check that verifies:

- `insomnia-pod --help` starts successfully.
- `insomnia` is installed and reaches argument parsing.
- bundled prompt resources and this Nix usage document are present in the output.

For full validation before handing changes to review, run:

```sh
nix build .#
nix flake check
cargo fmt --check
```

This packaging change does not require provider credentials. A Rust `cargo check` is only needed if Rust source or runtime path semantics are changed.

## Known limitations

- The package currently installs the TUI and Pod CLI only; development-only wrappers from `devshell.nix` are not part of the installable package.
- The TUI does not currently expose a conventional `--help` / `--version` CLI path, so the package smoke check uses an argument-parse failure path for the TUI rather than launching an interactive session.
- Bundled resources are installed under `share/insomnia/resources/` for packaging completeness and inspection. Built-in prompt/resource loading remains governed by the existing application code and user/project override rules.
