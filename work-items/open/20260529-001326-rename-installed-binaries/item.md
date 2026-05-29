---
id: 20260529-001326-rename-installed-binaries
slug: rename-installed-binaries
title: Rename installed binaries
status: open
kind: task
priority: P2
labels: [cli, packaging, tui, pod]
created_at: 2026-05-29T00:13:26Z
updated_at: 2026-05-29T00:13:26Z
assignee: null
legacy_ticket: null
---

## Background

The workspace crate names `tui` and `pod` are useful internally, but the installed command names are too generic for user environments. `tui` does not identify the application, and `pod` collides with common terminology and other tooling.

Use application-specific binary names for installed commands:

- `insomnia`: the main terminal UI / user entrypoint, currently built from the `tui` crate.
- `insomnia-pod`: the Pod CLI/runtime command, currently built from the `pod` crate.

This is a command name change, not a crate rename. Keep the Rust crate/package names `tui` and `pod` unless there is a separate design decision to rename crates.

## Requirements

- Rename Cargo binary outputs:
  - `crates/tui` binary name becomes `insomnia`.
  - `crates/pod` binary name becomes `insomnia-pod`.
- Do not add legacy `tui` / `pod` installed aliases unless a concrete internal dependency requires it and is documented.
- Update Nix packaging to install and check the new binary names.
  - `$out/bin/insomnia`
  - `$out/bin/insomnia-pod`
- Update flake apps to use the new command names.
  - default app should run `insomnia`.
  - expose app entries for `insomnia` and `insomnia-pod`.
- Update docs that instruct users to run `tui` / `pod` as installed commands.
  - Keep references to crate/package names where they are explicitly Cargo package names, e.g. `cargo check -p tui`.
  - Prefer `cargo run -p tui -- ...` in development docs if referring to crate-based development invocation, but installed usage should use `insomnia`.
- Audit code/tests/scripts for assumptions that installed binary names are `tui` or `pod`.
  - Internal runtime process spawning must still work.
  - If code intentionally uses Cargo package names, leave them unchanged.
- Keep CLI semantics unchanged except for command names.

## Acceptance criteria

- `cargo build -p tui -p pod` produces runnable binaries named `insomnia` and `insomnia-pod`.
- `cargo run -p tui -- --help` and `cargo run -p pod -- --help` still work as development package invocations.
- Installed/Nix package smoke checks look for `insomnia` and `insomnia-pod`, not `tui` and `pod`.
- `flake.nix` app outputs use the new binary names.
- User-facing docs no longer tell users to run installed commands as `tui` / `pod`.
- No legacy aliases are installed unless explicitly justified.
- `cargo fmt --check`
- Focused cargo checks/tests for affected crates, at least `cargo check -p tui -p pod`.
- Nix validation that does not require network where possible, e.g. `nix flake check --no-build`.

## Out of scope

- Renaming crates/packages from `tui` / `pod`.
- Changing CLI argument semantics.
- Changing Pod protocol or socket behavior.
- Publishing or Home Manager module changes.
