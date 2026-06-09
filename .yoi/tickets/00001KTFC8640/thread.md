<!-- event: create author: yoi ticket at: 2026-06-06T21:08:32Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T21:09:49Z -->

## Plan

Preflight result: `implementation-ready` cleanup.

The workspace panel now owns the Ticket/Intake/Orchestrator user-facing route, so the old single-Pod TUI `:ticket ...` command family should be removed rather than kept as fallback. Keep the shared role launcher because `yoi panel` uses it; remove only the TUI command surface/runtime handling and active docs/tests.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-06T21:28:39Z status: approve -->

## Review: approve

External reviewer approved the implementation with no requested changes.

Review summary:
- TUI `ticket` command registration/parser/handler was removed.
- `CommandAction::TicketRole`, `TicketRoleCommand`, pending local action plumbing, and the single-Pod local role-launch path were removed.
- Shared `client::ticket_role` code remains intact for `yoi panel`.
- `yoi panel`, `yoi ticket ...`, Ticket tools, and workflows remain intact.
- Active docs now direct users to `yoi panel`, Ticket tools/workflows, and `yoi ticket ...`; the remaining `:ticket ...` mention is an explicit unsupported note.
- Parser coverage treats `ticket intake ...` as unknown.
- No `--multi` reintroduction.


---

<!-- event: close author: hare at: 2026-06-06T21:28:39Z status: closed -->

## Closed

Removed the obsolete single-Pod TUI `:ticket ...` command surface.

Changes:
- Removed TUI command registration/parsing/handling for `:ticket intake`, `:ticket route`, `:ticket investigate`, `:ticket implement`, and `:ticket review`.
- Removed `CommandAction::TicketRole`, `TicketRoleCommand`, pending command-action plumbing, and the single-Pod runtime Ticket role launch path.
- Kept shared `client::ticket_role` launcher code because `yoi panel` uses it for Orchestrator/Intake launches.
- Kept `yoi panel`, `yoi ticket ...`, Ticket tools, and workflows intact.
- Updated active docs to direct users to `yoi panel`, Ticket tools/workflows, and `yoi ticket ...` instead of `:ticket ...`.
- Left an explicit unsupported note for single-Pod `:ticket ...` command mode.
- Updated parser tests so `ticket intake ...` is unknown/unsupported.
- Did not reintroduce `--multi`.

Validation after merge:
- `cargo test -p tui command`
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p yoi panel`
- `cargo test -p client ticket_role`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

Remaining active-reference matches are intentional: `yoi ticket review`, panel/client `ticket_role` usage, unsupported-note/test coverage, historical report text, and non-command identifiers such as `ticket_enabled` / `builtin:ticket`.

External review approved with no requested changes.


---
