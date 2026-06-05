<!-- event: create author: tickets.sh at: 2026-05-31T00:55:57Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-31T00:56:38Z -->

## Decision

Decision note from discussion:

- Short-term direction: keep adding headless commands to the existing user-facing `insomnia` binary owned by the current `tui` crate.
- Product preference: a single standalone `insomnia` binary is easier to distribute and explain than many small binaries.
- Known tradeoff: headless commands inherit TUI dependencies such as ratatui/crossterm. This is acceptable until binary size/startup/runtime memory is measured as a real problem.
- Internal structure should still separate headless command dispatch from terminal/TUI initialization.
- Future cleanup: consider renaming the Cargo package/crate from `tui` to `insomnia`; treat it as part of this ticket only if the scope remains contained.


---

<!-- event: decision author: hare at: 2026-05-31T04:32:30Z -->

## Decision

Revised decision from user discussion:

- The intended single-binary work is not merely “put headless subcommands in the existing `insomnia` binary”; it is to migrate the current `insomnia` + `insomnia-pod` two-binary architecture toward one primary executable.
- Pod runtime should remain a separate process. The unification is at the executable/entrypoint/packaging level.
- `insomnia-pod` does not need to remain as a long-term alias. It was not designed as a human-facing command.
- Prefer a normal subcommand `insomnia pod ...` for Pod runtime startup instead of a hidden `__pod-runtime` command.
- `tui` package/crate rename remains separate from binary unification unless it becomes necessary.

Initial implementation should start by extracting the Pod runtime into a library entrypoint and adding `insomnia pod ...`; subsequent steps can migrate spawn defaults and remove `insomnia-pod` from packaging.


---

<!-- event: close author: hare at: 2026-05-31T12:15:50Z status: closed -->

## Closed

Completed the umbrella migration from the previous two-command installed layout toward a single primary `insomnia` executable.

Completed phases:
- `insomnia-pod-subcommand-runtime`: moved Pod runtime startup behind `pod::entrypoint` and added `insomnia pod ...` dispatch.
- `spawn-through-insomnia-pod-subcommand`: changed internal spawn/restore defaults to typed runtime command resolution using current executable plus the `pod` prefix argument.
- `remove-insomnia-pod-binary`: removed the long-term `insomnia-pod` binary/package/devshell/flake output.
- Follow-up cleanup removed `INSOMNIA_POD_COMMAND` and `INSOMNIA_RESOURCE_DIR`, keeping the runtime command/config surface narrower.

Outcome:
- The installed package exposes `bin/insomnia` only.
- `insomnia pod ...` is the Pod runtime entrypoint; Pods remain separate processes.
- Internal spawn/restore uses typed command construction rather than shell string parsing.
- `insomnia-pod` is not kept as a compatibility alias.
- Headless `insomnia memory lint` behavior remains part of the `insomnia` CLI surface.

Validation/evidence across completed phases included:
- `insomnia pod --help`
- focused parser/spawn/restore tests
- `cargo fmt --check`
- `cargo check -p tui -p pod -p client`
- `nix build .#insomnia`
- checks that `bin/insomnia-pod` is absent
- `./tickets.sh doctor`
- `git diff --check`

Follow-up intentionally remains separate:
- `insomnia-crate-cli-owner` now tracks the architectural cleanup where the `insomnia` crate owns the product CLI/binary entrypoint and `tui` becomes a library implementation crate. That is not required for this umbrella's original single-installed-binary migration to be complete.


---
