---
id: 20260531-043239-insomnia-pod-subcommand-runtime
slug: insomnia-pod-subcommand-runtime
title: CLI: add insomnia pod runtime entrypoint
status: closed
kind: task
priority: P2
labels: [cli, pod, nix]
created_at: 2026-05-31T04:32:39Z
updated_at: 2026-05-31T04:50:14Z
assignee: null
legacy_ticket: null
---

## Background

Parent/umbrella ticket: `single-binary-insomnia-cli`.

The project currently has two installed command names:

- `insomnia` from the `tui` package for user-facing CLI/TUI/headless commands;
- `insomnia-pod` from the `pod` package for Pod runtime processes.

The target direction is one primary executable, `insomnia`, with `insomnia pod ...` as the Pod runtime entrypoint. Pod runtime remains a separate process when spawned; only the binary/entrypoint is unified.

This ticket is the first implementation step. It should add the new `insomnia pod ...` runtime entrypoint and share Pod runtime startup code, without yet requiring all internal spawn paths or packaging to drop `insomnia-pod` in the same diff.

## Requirements

- Extract the current `crates/pod/src/main.rs` runtime startup into a library-callable entrypoint in the `pod` crate.
  - Keep existing Pod runtime flags/semantics.
  - Preserve `INSOMNIA-READY` stderr handshake and detached process behavior.
- Keep the existing `insomnia-pod` binary only as a temporary transition wrapper if needed for tests/current spawn defaults.
  - Do not present it as the long-term compatibility alias.
  - Do not change Nix installed command set in this ticket unless it is mechanically necessary.
- Add `insomnia pod ...` to the existing `insomnia` binary in the `tui` crate.
  - `insomnia pod --help` should reach the Pod runtime parser/help.
  - `insomnia pod <runtime flags>` should invoke the same library entrypoint as the old runtime path.
  - This means `insomnia pod` becomes reserved as a subcommand; a Pod literally named `pod` can still be addressed through explicit `--pod pod` if needed.
- Preserve existing user-facing TUI/CLI behavior for non-`pod` commands:
  - `insomnia memory lint` remains headless and returns before terminal/TUI/Pod side effects;
  - existing `insomnia <pod-name>` / `--pod` / `--multi` / resume behavior remains unchanged except for the reserved `pod` subcommand.
- Do not yet switch `client::spawn_pod`, `SpawnPod`, or `RestorePod` default command resolution to `insomnia pod` unless doing so is small and fully tested.
  - If not switched, create/fill a follow-up note for command resolution `program + prefix_args` migration.
- Do not rename the `tui` package/crate in this ticket.

## Non-goals

- Removing `insomnia-pod` from packaging in this first step.
- Keeping `insomnia-pod` as a long-term alias.
- Merging Pod controller into the TUI process.
- Reworking Pod protocol or CLI UX beyond the new runtime subcommand.
- Feature-gating TUI dependencies.

## Acceptance criteria

- `insomnia pod --help` works and uses the Pod runtime parser/help.
- `insomnia-pod --help` still works if the temporary binary remains.
- `insomnia memory lint` still runs headlessly without TUI/raw-terminal/Pod startup side effects.
- Existing TUI parser tests are updated for the reserved `pod` subcommand and preserve other positional Pod-name behavior.
- Pod runtime startup code has one shared library path rather than duplicated main logic.
- Follow-up work for switching internal spawn defaults and removing `insomnia-pod` packaging is recorded if not completed here.
- `cargo fmt --check`, focused `cargo test` for affected TUI/Pod CLI parsing, `cargo check -p tui -p pod -p client`, `./tickets.sh doctor`, and `git diff --check` pass.
