---
id: 20260531-054728-remove-insomnia-pod-binary
slug: remove-insomnia-pod-binary
title: CLI: remove insomnia-pod installed/runtime alias
status: closed
kind: task
priority: P2
labels: [cli, pod, nix, docs]
created_at: 2026-05-31T05:47:28Z
updated_at: 2026-05-31T06:10:39Z
assignee: null
legacy_ticket: null
---

## Background

Parent/umbrella ticket: `single-binary-insomnia-cli`.

Previous phases have already:

- added `insomnia pod ...` as the Pod runtime entrypoint;
- moved internal spawn/restore defaults to typed `current_exe() + ["pod"]` command resolution;
- left `insomnia-pod` as a temporary old binary/package output.

The user decision is that `insomnia-pod` does not need to remain as a compatibility alias. It was never designed as a human-facing command. The next phase is to remove/demote it so the installed package exposes one primary runtime executable: `insomnia`.

## Requirements

- Remove the long-term `insomnia-pod` installed/runtime alias.
- Prefer removing the `insomnia-pod` binary target from the `pod` crate if no active tests/build paths require it.
  - The `pod` crate should remain as a library crate providing the Pod runtime entrypoint used by `insomnia pod ...`.
  - If a temporary binary target must remain for a narrow reason, document exactly why and do not install/expose it.
- Update Nix packaging:
  - build/install only the user-facing `insomnia` binary as the package command;
  - remove `apps.insomnia-pod` from `flake.nix`;
  - update install checks to use `insomnia pod --help` instead of `insomnia-pod --help`.
- Update devshell behavior:
  - remove the `insomnia-pod` wrapper if internal callers no longer need it;
  - preserve the “do not pollute `INSOMNIA-READY` stderr handshake with cargo output” property for dev workflows if a wrapper remains necessary;
  - document any remaining dev override path (`INSOMNIA_POD_COMMAND`) if still relevant.
- Update docs that describe installed commands / Nix usage / profile manifest CLI examples:
  - replace user-facing `insomnia-pod ...` examples with `insomnia pod ...` where current behavior is intended;
  - avoid rewriting historical work-item artifacts;
  - leave clearly historical docs alone unless they are current user docs.
- Preserve runtime behavior:
  - `insomnia pod --help` works;
  - Pod spawning/restoration still uses `insomnia pod ...` by default;
  - `INSOMNIA_POD_COMMAND` override behavior remains executable-only if still supported, or is intentionally removed with tests/docs updated.
- Do not rename the `tui` package/crate in this ticket.
- Do not merge Pod controller into the TUI process.

## Non-goals

- Crate/package rename from `tui` to `insomnia`.
- Removing the `pod` library crate.
- Changing Pod runtime flags/profile/manifest semantics.
- Changing Pod protocol.
- Large CLI UX redesign.

## Acceptance criteria

- Installed/Nix package exposes `bin/insomnia` as the primary command and no longer exposes `bin/insomnia-pod`.
- `nix build .#insomnia` and install checks pass using `insomnia pod --help`.
- `flake.nix` no longer advertises `apps.insomnia-pod`.
- `insomnia pod --help` and relevant Pod runtime parser tests pass.
- Internal spawn/restore tests still pass after removing the old binary output.
- Current docs no longer instruct users to run `insomnia-pod` as the normal runtime command; use `insomnia pod` instead.
- `cargo fmt --check`, focused pod/tui/client/nix tests or checks, `cargo check -p tui -p pod -p client`, `./tickets.sh doctor`, and `git diff --check` pass.
