# Implementation report: ticket-local-files-backend

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-local-files-backend`
- Branch: `work/ticket-local-files-backend`

## Commit

- `740b017 feat: add local ticket backend`

## Summary

Added a new low-level `ticket` workspace crate that exposes a typed Ticket domain, `TicketBackend` trait, and `LocalTicketBackend` over the existing `work-items/` local markdown storage.

The implementation preserves the current local file layout and `tickets.sh` compatibility while making Ticket operations available as Rust API for later built-in tools, Intake workflow, and Orchestrator routing work.

## Public API / module summary

- `Ticket`, `TicketSummary`, `TicketMeta`, `TicketEvent`, `TicketReview`, `TicketArtifactRef`
- `TicketStatus`, `TicketEventKind`, `TicketReviewStatus`, `TicketKind`, `TicketPriority`
- `TicketIdOrSlug`, `TicketFilter`, `NewTicket`, `NewTicketEvent`
- `TicketBackend` trait
- `LocalTicketBackend`
- `TicketDoctorReport` / `TicketDoctorDiagnostic`

Implemented backend operations:

- `list`
- `show`
- `create`
- `add_event`
- `review`
- `set_status`
- `close`
- `doctor`

## Changed files

- `Cargo.lock`
- `Cargo.toml`
- `package.nix`
- `crates/ticket/Cargo.toml`
- `crates/ticket/src/lib.rs`

## Compatibility

- Keeps `work-items/{open,pending,closed}/<id>/item.md`, `thread.md`, `artifacts/`, and closed `resolution.md` layout.
- Rust-created tickets pass `tickets.sh doctor` in tempdir tests.
- `tickets.sh`-created tickets are readable by the Rust backend in tempdir tests.
- `tickets.sh` can mutate Rust-created tickets in tempdir tests.
- Real repository `./tickets.sh doctor` passed.

## Safety / scope

- New crate is lower-level and independent of `pod`, `tui`, Intake, Orchestrator routing, and scheduler code.
- No Pod tools or UI were added.
- No storage directory rename was introduced.
- Writes are constrained to configured backend root paths.
- Local backend uses a `.ticket-backend.lock` file for Rust backend caller coordination.
- File rewrites use temp files in the destination directory followed by rename where practical.

## Validation

Coder-reported validation passed:

- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `git diff --cached --check`
- `./tickets.sh doctor`
- `nix build .#yoi`

Reviewer-rerun validation passed:

- `cargo test -p ticket --no-run`
- `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- `nix build .#yoi --no-link`

## Review status

External sibling reviewer approved with no blockers.

Non-blocker follow-ups:

- Event references are modeled but not persisted/parsed in `thread.md` yet.
- Extension enum variants should remain read-tolerant, while local write paths should avoid emitting values that `tickets.sh doctor` rejects unless the format is intentionally extended.
- Backend locking coordinates Rust backend callers, not concurrent direct `tickets.sh` writes.
- The inherited `thread.md` `---` separator remains ambiguous for event bodies containing standalone `---` lines.

## Ready for merge

Yes.
