<!-- event: create author: tickets.sh at: 2026-06-05T20:30:06Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T20:34:35Z -->

## Plan

# Delegation intent: yoi ticket CLI parity

## Intent

Add `yoi ticket ...` CLI operations over the Rust Ticket backend so direct Ticket mutation is owned by the `yoi` binary rather than `tickets.sh`.

This is the first step in the Yoi-local Ticket backend migration. `tickets.sh` remains in place for this ticket only as transitional compatibility and validation reference.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/yoi-ticket-cli-parity`
- branch: `work/yoi-ticket-cli-parity`

## Requirements

Implement these subcommands in the top-level `yoi` binary crate:

- `yoi ticket create --title "..." [--slug slug] [--kind task] [--priority P2] [--label a,b]`
- `yoi ticket list [--status open|pending|closed|all]`
- `yoi ticket show <id-or-slug>`
- `yoi ticket comment <id-or-slug> [--role comment|plan|decision|implementation_report] [--file path|--message text]`
- `yoi ticket review <id-or-slug> --approve|--request-changes [--file path|--message text]`
- `yoi ticket status <id-or-slug> open|pending|closed`
- `yoi ticket close <id-or-slug> [--resolution text|--file path]`
- `yoi ticket doctor`

Use `crates/ticket` backend APIs directly. Do not shell out to `tickets.sh`.

## Current code map

- `crates/yoi/src/main.rs`
  - Owns top-level CLI parsing and dispatch.
  - Existing subcommands include `pod`, `keys`, and `memory lint`.
  - Add `ticket` parsing/dispatch here or in a new module such as `crates/yoi/src/ticket_cli.rs`.
- `crates/yoi/Cargo.toml`
  - Add dependency on `ticket` if needed.
- `crates/ticket/src/lib.rs`
  - Ticket domain/backend APIs: `LocalTicketBackend`, `TicketBackend`, `NewTicket`, `NewTicketEvent`, `TicketReview`, status/review/event types, doctor reports.
- `crates/ticket/src/config.rs`
  - Workspace ticket config and backend root defaults. Use if it helps locate backend root; otherwise current active storage is still `work-items/` for this ticket.
- `tickets.sh`
  - Compatibility/reference only. Do not invoke it from Rust code.

## Scope notes

- Active storage stays where it is for this ticket (`work-items/` unless config already directs otherwise).
- Keep `tickets.sh` in place.
- CLI output does not need exact `tickets.sh` formatting, but should be understandable and stable enough for tests.
- Prefer concise plain text output for human CLI; JSON is not required unless already easy.
- `yoi ticket doctor` should be usable as the future validation command.

## Non-goals

- Moving storage to `.yoi/tickets/`.
- Removing `tickets.sh`.
- Adding external tracker backends.
- TUI changes.
- Scheduler/lease/queue.
- Broad CLI framework refactor.

## Edge cases / validation expectations

- `comment` / `review` should accept exactly one body source: `--file` or `--message`, or provide a clear error.
- `review` requires exactly one of `--approve` / `--request-changes`.
- `close` requires resolution text/file or a clear error if the backend cannot close without a resolution.
- `status closed` should preserve backend semantics. If direct status-to-closed cannot write a resolution, either reject it with guidance to use `close` or keep parity only if existing backend supports it safely.
- `doctor` should print diagnostics and return non-zero on failures.
- `--help` for `yoi ticket` should document subcommands.

## Validation

Run at least:

- focused `cargo test -p yoi ticket` tests;
- `cargo test -p ticket`;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `./tickets.sh doctor`;
- built binary `yoi ticket doctor` against the repository if possible.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- implemented command surface;
- backend root resolution behavior;
- tests/validation results;
- known compatibility gaps from `tickets.sh`, if any;
- whether ready for external review.


---

<!-- event: review author: hare at: 2026-06-05T20:56:41Z status: approve -->

## Review: approve

# External review: yoi-ticket-cli-parity

## 1. Result

approve

## 2. Summary of implementation

The implementation adds a new `yoi ticket ...` command surface in the product `yoi` binary crate, with parsing and dispatch in `crates/yoi/src/main.rs` and the command implementation in `crates/yoi/src/ticket_cli.rs`.

The CLI resolves the local Ticket backend through `ticket::config::TicketConfig::load_workspace`, defaults to `<cwd>/work-items` when no `.yoi/ticket.config.toml` is present, and then calls `LocalTicketBackend` / `TicketBackend` operations directly. I found no production-path shell-out to `tickets.sh`; the only `tickets.sh` process invocations found are in `crates/ticket` compatibility tests.

The changed files are scoped to the `yoi` CLI crate, the `ticket` backend doctor reporting shape, and necessary dependency/package hash updates. I did not find storage migration, `tickets.sh` removal, TUI changes, scheduler/lease work, or broad refactoring.

## 3. Requirement-by-requirement assessment

- `yoi ticket` subcommands cover create/list/show/comment/review/status/close/doctor: satisfied. `TicketCommand` contains all requested operations and `help_text()` documents them.
- Product CLI ownership in the `yoi` binary crate: satisfied. `main.rs` adds `Mode::Ticket` and dispatches `ticket` before normal TUI argument handling.
- Uses Rust Ticket backend APIs directly, no `tickets.sh` shell-out: satisfied for the product CLI path. `ticket_cli.rs` calls `LocalTicketBackend` and `TicketBackend` methods directly.
- Backend root resolution and active storage: satisfied for this ticket. The CLI uses `.yoi/ticket.config.toml` when present and otherwise defaults to `<cwd>/work-items`, preserving current local storage rather than moving to `.yoi/tickets`.
- `status closed` safety: satisfied. `status closed` is rejected with guidance to use `yoi ticket close <ticket> --resolution <text>`.
- `comment`, `review`, and `close` body source validation: satisfied. The parser requires exactly one body source and rejects conflicting/missing inputs; `review` also requires exactly one result flag.
- Doctor success/failure behavior: satisfied for errors. `TicketCliStatus::Failure` maps to a failing process exit code, and diagnostics are printed when errors are present.
- Human-useful output: broadly satisfied. Output is concise tabular/plain text for create/list/status and readable Markdown-like output for show.
- Bounded output: partially satisfied only by the natural size of the local backend; `show`, `list`, and doctor diagnostics do not impose explicit limits. I classify this as a follow-up because the requested CLI parity is still implemented and the current compatibility CLI is also not explicitly bounded.
- Tests in temp roots/fixtures: satisfied in implementation. `ticket_cli.rs` exercises core operations in `TempDir`, including configured backend root behavior and validation edge cases; `crates/ticket` keeps compatibility tests.
- `Cargo.lock` / `package.nix`: necessary and safe on inspection. Adding `ticket` to the `yoi` crate requires the lockfile package dependency update, and the Nix cargo hash update is expected from the Cargo metadata/source change.
- Non-goals: satisfied. I found no `.yoi/tickets` migration, `tickets.sh` removal, TUI change, scheduler/lease addition, or broad refactor.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- `yoi ticket doctor` suppresses warning-only diagnostics because `doctor()` returns early with `doctor: ok` when `report.error_count() == 0`. If backend warnings are intended to be user-visible, the CLI should print warnings while still exiting successfully. This is not a merge blocker because the old `tickets.sh doctor` only had errors and the required failure behavior for errors is present.
- The CLI does not explicitly bound `show`, `list`, or diagnostic output. Consider adding limits later if this command is expected to be safe for very large Ticket stores or oversized thread bodies.
- The generic body-source error text says `--message/--resolution` for all commands, so comment/review errors mention `--resolution` even though that flag is only for close. The validation is correct; the wording can be improved in follow-up.

## 6. Validation assessed or rerun

Rerun/read-only validation:

- `git diff --check develop...HEAD` — passed with no output.
- `./tickets.sh doctor` — passed: `doctor: ok`.
- `git status --short && git branch --show-current && git rev-parse --short=12 HEAD` — confirmed branch `work/yoi-ticket-cli-parity` at `4d5068ba3baf`; no worktree dirt was reported before this review artifact was written.
- `git grep -n "tickets.sh\|std::process::Command\|Command::new" -- crates/yoi crates/ticket` — no production CLI shell-out found; only `crates/ticket` compatibility tests invoke `tickets.sh`.

Inspected:

- Ticket and delegation intent.
- `crates/yoi/src/ticket_cli.rs`.
- `crates/yoi/src/main.rs`.
- `crates/ticket/src/lib.rs` diff.
- `crates/ticket/src/config.rs` backend default/config behavior.
- `crates/yoi/Cargo.toml`, `Cargo.lock` diff, and `package.nix` hash change.

Not rerun: `cargo test`, `cargo check`, `cargo fmt --check`, built `yoi ticket doctor`, or `nix build .#yoi`; I stayed to read-only validation commands as requested for this external sibling review.

## 7. Residual risk

The main residual risk is not semantic parity but operational validation: I did not rerun the Rust or Nix builds, so the final merge owner should rely on the coder's validation evidence or rerun the full acceptance suite before merging. There is also minor UX risk around unbounded `show/list` output and warning-only doctor output being hidden.


---

<!-- event: implementation_report author: hare at: 2026-06-05T20:56:42Z -->

## Implementation report

# Implementation report: yoi-ticket-cli-parity

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/yoi-ticket-cli-parity`
- Branch: `work/yoi-ticket-cli-parity`

## Commit

- `4d5068b feat: add yoi ticket CLI`

## Summary

Added `yoi ticket ...` subcommands to the product `yoi` binary crate, backed directly by the Rust Ticket backend APIs.

The implementation does not shell out to `tickets.sh`. Active storage remains unchanged for this ticket: when `.yoi/ticket.config.toml` is absent, the CLI uses the current default `<workspace>/work-items`.

## Implemented command surface

- `yoi ticket create --title <title> [--slug <slug>] [--kind <kind>] [--priority P2] [--label a,b]`
- `yoi ticket list [--status open|pending|closed|all]`
- `yoi ticket show <id-or-slug>`
- `yoi ticket comment <id-or-slug> [--role comment|plan|decision|implementation_report] (--file <path>|--message <text>)`
- `yoi ticket review <id-or-slug> (--approve|--request-changes) (--file <path>|--message <text>)`
- `yoi ticket status <id-or-slug> <open|pending|closed>`
- `yoi ticket close <id-or-slug> (--resolution <text>|--file <path>)`
- `yoi ticket doctor`

`yoi ticket status ... closed` is intentionally rejected with guidance to use `yoi ticket close`, because status-only close would not write `resolution.md`.

## Backend behavior

- Uses `ticket::config::TicketConfig::load_workspace(<cwd>)` to resolve backend root.
- Missing `.yoi/ticket.config.toml` defaults to `<cwd>/work-items`.
- Relative configured roots resolve against the workspace.
- Malformed config fails through the existing config loader.
- No storage migration to `.yoi/tickets` is included.

## Changed files

- `Cargo.lock`
- `crates/ticket/src/lib.rs`
- `crates/yoi/Cargo.toml`
- `crates/yoi/src/main.rs`
- `crates/yoi/src/ticket_cli.rs`
- `package.nix`

## Review status

External sibling reviewer approved with no blockers.

Non-blocker follow-ups:

- `yoi ticket doctor` currently hides warning-only diagnostics because it prints `doctor: ok` when `error_count() == 0`.
- `show`, `list`, and doctor diagnostic output are not explicitly bounded.
- Body-source error text is generic and can mention `--resolution` for comment/review commands.

## Validation

Coder-reported validation passed:

- `cargo test -p yoi ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `git diff --cached --check`
- `./tickets.sh doctor`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`

Reviewer-rerun validation passed:

- `git diff --check develop...HEAD`
- `./tickets.sh doctor`
- production CLI grep found no shell-out to `tickets.sh`.

## Ready for merge

Yes.


---

<!-- event: close author: hare at: 2026-06-05T20:58:21Z status: closed -->

## Closed

Yoi ticket CLI parity is complete and merged.

Implementation:

- `4d5068b feat: add yoi ticket CLI`
- merge commit: `4ba1b2f merge: add yoi ticket cli`

Summary:

- Added `yoi ticket ...` subcommands to the product `yoi` binary.
- Implemented:
  - `yoi ticket create`
  - `yoi ticket list`
  - `yoi ticket show`
  - `yoi ticket comment`
  - `yoi ticket review`
  - `yoi ticket status`
  - `yoi ticket close`
  - `yoi ticket doctor`
- The CLI uses `crates/ticket` backend APIs directly.
- The CLI does not shell out to `tickets.sh`.
- Active storage remains unchanged for this ticket: absent `.yoi/ticket.config.toml`, the backend defaults to `<workspace>/work-items`.
- `yoi ticket status ... closed` is intentionally rejected with guidance to use `yoi ticket close`, because status-only close would not write `resolution.md`.

Review:

- External sibling reviewer approved with no blockers.
- Non-blocker follow-ups:
  - `yoi ticket doctor` currently hides warning-only diagnostics when there are no errors.
  - `show`, `list`, and doctor diagnostic output are not explicitly bounded.
  - Body-source error text is generic and can mention `--resolution` for comment/review commands.

Post-merge validation passed:

- `cargo test -p yoi ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`

This clears the CLI prerequisite for the next migration step: `builtin-yoi-local-ticket-backend-config`.


---
