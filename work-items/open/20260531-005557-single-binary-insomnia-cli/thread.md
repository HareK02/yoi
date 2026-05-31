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
