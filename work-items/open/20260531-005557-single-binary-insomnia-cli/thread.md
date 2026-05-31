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
