---
title: "ワークスペースのメモリーをLintするヘッドレスCLI"
state: "closed"
created_at: "2026-05-27T00:00:19Z"
updated_at: "2026-05-31T02:15:17Z"
---

## Background

The memory linter currently exists as library/pre-write validation used by memory tools, but there is no headless command to check all existing workspace memory/knowledge records at once. This makes it hard to validate `.insomnia/memory` and `.insomnia/knowledge` before commits, migrations, or manual edits.

The installed user-facing binary is currently produced by the `tui` crate as `insomnia`. It is acceptable for this ticket to add the headless lint command to that crate/binary instead of introducing a separate binary. A future rename from `tui` crate to `insomnia`, or a more explicit single-binary CLI structure, can be handled separately.

## Requirements

- Add a headless CLI mode to the existing `insomnia` binary in the `tui` crate.
- Preferred invocation shape: `insomnia memory lint [--workspace <PATH>] [--json] [--warnings-as-errors]`.
  - `insomnia memory` without `lint` should remain available as a normal positional Pod name if possible.
  - If this shape is awkward with the current parser, keep the command unambiguous and document the chosen shape in tests/help text.
- Default workspace root is the current working directory.
- `--workspace <PATH>` overrides the workspace root passed to `memory::WorkspaceLayout::new`.
- Lint all existing records classified by `memory::WorkspaceLayout`:
  - `.insomnia/memory/summary.md` when present;
  - `.insomnia/memory/decisions/*.md`;
  - `.insomnia/memory/requests/*.md`;
  - `.insomnia/knowledge/*.md`.
- Do not lint subsystem-owned opaque trees such as `.insomnia/memory/_staging`, `_logs`, `_usage`.
- Use the existing `memory::Linter` and `WriteMode::Update` for existing files so the CLI matches tool pre-write validation semantics without triggering create-only duplicate slug checks on the file itself.
- Print a deterministic, human-readable report by default:
  - file path;
  - errors;
  - warnings;
  - summary counts.
- Exit status:
  - `0` if no errors, and no warnings when `--warnings-as-errors` is set;
  - `1` if lint errors are found, or warnings are found with `--warnings-as-errors`;
  - `2` for CLI usage / I/O / unexpected runtime failures.
- `--json` may be simple but should be machine-readable and stable enough for scripts: include workspace, files, errors, warnings, and counts.
- The command must not start a Pod, connect to sockets, enter raw terminal mode, or mutate files.

## Non-goals

- Renaming the `tui` crate to `insomnia`.
- Adding a separate installed binary.
- Linting Workflow files; workflow linting can be a future command.
- Auto-fixing memory/knowledge records.
- Changing memory schema/linter rules.

## Acceptance criteria

- `insomnia memory lint` runs headlessly against the current directory and reports existing memory/knowledge lint results.
- `insomnia memory lint --workspace <PATH>` works in tests/fixtures.
- The command exits non-zero for lint errors.
- `--warnings-as-errors` makes warnings fail.
- `--json` returns valid JSON containing counts and per-file diagnostics.
- Existing Pod/TUI argument parsing behavior remains covered by tests, especially positional Pod names and `--multi`/`--resume` conflicts.
- `cargo fmt --check`, focused `cargo test -p tui` tests, `cargo check -p tui`, `./tickets.sh doctor`, and `git diff --check` pass.
