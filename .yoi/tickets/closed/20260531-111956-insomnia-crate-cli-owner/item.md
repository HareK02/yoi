---
id: 20260531-111956-insomnia-crate-cli-owner
slug: insomnia-crate-cli-owner
title: 'CLI: make insomnia crate own binary entrypoint and CLI dispatch'
status: closed
kind: task
priority: P2
labels: [cli, tui, pod, architecture]
created_at: 2026-05-31T11:19:56Z
updated_at: 2026-05-31T13:20:02Z
assignee: null
---

## Background

The single-binary migration currently exposes the installed command as `insomnia`, but the binary target and CLI dispatch are still owned by the `tui` package (`cargo run -p tui -- ...`, `crates/tui/src/main.rs`). The helper crate now named `insomnia` was originally introduced for Pod runtime command resolution, which creates a responsibility mismatch.

The desired architecture is that the `insomnia` crate owns the product CLI and binary entrypoint. The `tui` and `pod` crates should be libraries with no `main.rs`, and CLI/tool subcommand routing should happen at the `insomnia` boundary.

## Requirements

- Move product CLI ownership to the `insomnia` crate:
  - binary target `insomnia` belongs to package/crate `insomnia`;
  - CLI argument parsing and top-level mode/subcommand dispatch live in `insomnia`;
  - development entrypoint becomes `cargo run -p insomnia -- ...`.
- Make `tui` a library implementation crate rather than binary/CLI owner.
  - `tui` should expose an API for launching the terminal UI with already-parsed options/configuration.
  - `tui` should not own `insomnia pod ...` dispatch or headless CLI tool routing.
- Keep `pod` as a library runtime crate with no dependency on `insomnia`.
  - `pod` may expose its runtime entrypoint/parser API for `insomnia pod ...`, but it must not know the product CLI crate.
- Move or relocate `PodRuntimeCommand` responsibility out of the current `insomnia` helper role if needed to avoid inverted dependencies.
  - `pod -> insomnia` is not acceptable.
  - Prefer passing the runtime command from the top-level CLI/orchestrating layer into TUI/client spawn paths.
  - If a shared type is still needed, place it in a lower-level crate whose responsibility is process/client mechanics, not in the product CLI crate by accident.
- CLI tools/headless commands should be routed by `insomnia`, not by `tui`.
  - Examples: `memory lint`, `pod ...`, future non-TUI commands.
- Preserve observable behavior:
  - installed command remains `insomnia`;
  - `insomnia -r`, `insomnia --multi`, `insomnia <podname>`, and `insomnia pod ...` keep their behavior;
  - no `insomnia-pod` alias is reintroduced;
  - Pod runtime remains a separate process.
- Do not add compatibility layers for `cargo run -p tui -- ...`; updating docs/dev commands is sufficient.

## Non-goals

- Renaming the user-facing command away from `insomnia`.
- Merging Pod runtime into the TUI process.
- Reintroducing `INSOMNIA_POD_COMMAND` or test-only env command overrides.
- Redesigning Pod protocol, profile semantics, or manifest semantics.
- Broad TUI module-layout refactors beyond what is necessary to make `tui` library-callable.

## Acceptance criteria

- `cargo run -p insomnia -- --help` and `cargo run -p insomnia -- pod --help` work as the main development entrypoints.
- Package `tui` no longer owns a binary `main.rs` entrypoint, or any remaining binary target is explicitly temporary and not the product CLI owner.
- No active code path requires `pod -> insomnia` or `tui -> insomnia` dependency.
- `insomnia` crate owns top-level CLI parsing/dispatch for TUI launch, Pod runtime launch, and headless CLI tools.
- Pod spawn/restore still launches runtime subprocesses as `insomnia pod ...` through typed command injection from the top-level owner.
- Nix/package outputs still expose only `bin/insomnia`.
- Current docs/dev instructions no longer tell developers to use `cargo run -p tui -- ...` as the primary product CLI path.
- Focused CLI/parser/spawn tests, `cargo fmt --check`, relevant `cargo check`/`cargo test`, `nix build .#insomnia` if packaging changed, `./tickets.sh doctor`, and `git diff --check` pass.
