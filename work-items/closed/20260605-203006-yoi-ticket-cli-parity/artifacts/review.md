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
