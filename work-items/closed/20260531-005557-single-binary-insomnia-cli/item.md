---
id: 20260531-005557-single-binary-insomnia-cli
slug: single-binary-insomnia-cli
title: CLI: migrate toward a single insomnia binary
status: closed
kind: task
priority: P2
labels: [cli, architecture, nix]
created_at: 2026-05-31T00:55:57Z
updated_at: 2026-05-31T12:15:50Z
assignee: null
legacy_ticket: null
---

## Background

The repository currently installs two command names:

- `insomnia` from the `tui` package: user-facing TUI/CLI entry point, including headless subcommands such as `insomnia memory lint`.
- `insomnia-pod` from the `pod` package: Pod runtime process entry point used by the TUI, restore flows, and `SpawnPod` to start detached Pod controller processes.

The desired direction is one primary installed executable, `insomnia`, that can act as both the user-facing CLI/TUI and the Pod runtime process entry point. Pod runtime should remain a separate process; the unification is about packaging/entrypoint, not merging TUI and Pod controller into one process.

The Pod runtime does not need a separate human-friendly `insomnia-pod` alias. It was never intended as a user-facing command. Prefer an explicit subcommand such as `insomnia pod ...` over a hidden `__pod-runtime` command.

## Requirements

- Plan and implement a staged migration toward a single primary installed `insomnia` executable.
- Target CLI shape:
  - `insomnia` / `insomnia <pod>` / `insomnia --pod ...` / `insomnia --multi` keep current user-facing behavior;
  - `insomnia memory lint ...` remains a headless command;
  - `insomnia pod ...` invokes the Pod runtime using the existing Pod runtime flags/semantics.
- Pod runtime remains a detached child process when spawned by TUI/RestorePod/SpawnPod.
- `insomnia-pod` is not kept as a long-term compatibility alias. If a transition step leaves it temporarily, the follow-up/removal boundary must be explicit.
- Refactor Pod runtime startup so both old and new entrypoints, if temporarily present, share one library entrypoint.
- Update internal spawn/restore paths to support executable + prefix args, so `insomnia pod ...` can replace executable-only `insomnia-pod ...` invocations.
- Preserve the `INSOMNIA-READY` stderr handshake and detached process behavior.
- Preserve headless command invariants: headless subcommands must not initialize ratatui/raw terminal, connect Pod sockets, or spawn Pod processes unless requested by that command.
- Preserve Nix/Home Manager packaging expectations and make the intended installed command set explicit.
- Evaluate `tui` package rename separately. It is not required for runtime unification and should not be mixed into early migration phases unless it becomes mechanically unavoidable.

## Suggested phases

1. Extract Pod runtime startup into a library entrypoint and add `insomnia pod ...` dispatch.
2. Extend Pod runtime command resolution to support `program + prefix_args` and switch TUI/RestorePod/SpawnPod defaults to `current_exe() + ["pod"]` where appropriate.
3. Update Nix/devshell/docs so the canonical installed runtime path is `insomnia pod ...`; remove `insomnia-pod` from installed package outputs when internal callers no longer need it.
4. Consider `tui` package/crate rename as a later cleanup only after binary/process unification is stable.

## Non-goals

- Running Pod controller inside the TUI process.
- Keeping `insomnia-pod` as a long-term public compatibility command.
- Feature-gating ratatui/crossterm for a headless-only build before there is a measured need.
- Large CLI UX redesign beyond the runtime entrypoint migration.
- Renaming `tui` package/crate in the first implementation step.

## Acceptance criteria

- The repository has a concrete staged plan and implementation tickets for moving Pod runtime startup under `insomnia pod`.
- First implementation step lands without changing unrelated CLI behavior.
- `insomnia pod --help` reaches the Pod runtime parser.
- Existing headless command behavior (`insomnia memory lint`) remains headless.
- Internal callers can be migrated from `insomnia-pod` to `insomnia pod` through typed command resolution rather than shell-string parsing.
- Follow-up/removal of `insomnia-pod` is explicit if not completed in the first implementation step.
- `cargo fmt --check`, relevant `cargo test`/`cargo check`, `./tickets.sh doctor`, and `git diff --check` pass for each step.
