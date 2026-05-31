<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:19Z -->

## Migrated

Migrated from TODO.md entry without a legacy ticket file. No legacy review file was present at migration time.

---

<!-- event: plan author: hare at: 2026-05-31T00:51:55Z -->

## Plan

Planning note:

- Keep this in the existing user-facing `insomnia` binary implemented by the `tui` crate. Do not add another installed command for this ticket.
- The command should be headless: parse args, lint files, print report, exit. It must not initialize terminal UI or connect to a Pod.
- `insomnia memory lint` is preferred, but `insomnia memory` alone should continue to be a valid Pod-name attach/create path if practical with the current parser.
- Use `memory::Linter` directly so CLI behavior tracks tool pre-write validation. Existing files should be linted with `WriteMode::Update`.
- Keep crate rename / single-binary architecture as future cleanup, not part of this ticket.


---
