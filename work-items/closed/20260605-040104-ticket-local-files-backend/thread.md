<!-- event: create author: tickets.sh at: 2026-06-05T04:01:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T04:24:20Z -->

## Plan

Preflight result: `implementation-ready`.

The Ticket naming and umbrella split are accepted. This ticket is scoped to the first implementation layer only: typed Ticket domain/backend code and LocalTicketBackend compatibility with current `work-items/` files.

Key decisions for implementation:

- Use `Ticket` as the public/domain concept name.
- Keep `work-items/` as the current local storage path.
- Add a lower-level Rust crate for the Ticket backend layer; do not put this in `pod` or `tui`.
- Preserve `tickets.sh` compatibility and do not remove or replace the script in this ticket.
- Treat readiness/action-required/risk fields as optional/extensible because existing tickets do not have a fully normalized schema.
- Keep Markdown/freeform bodies; enforce mechanical consistency and safe mutation, not rigid body sections.
- No Pod tools, Intake workflow, Orchestrator routing, TUI UI, external tracker backend, or scheduler in this ticket.

The detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-05T04:44:08Z status: approve -->

## Review: approve

# Review: ticket-local-files-backend

## 1. Result

approve

## 2. Summary of implementation

The implementation adds a new low-level `ticket` workspace crate with a typed Ticket domain, `TicketBackend` trait, and a filesystem-backed `LocalTicketBackend` for the existing `work-items/` directory layout. The crate is independent of `pod`, `tui`, scheduler/orchestrator, and intake UI/runtime code. It models tickets, events, reviews, statuses, artifacts, and doctor diagnostics, and provides operations for list/show/create/add_event/review/set_status/close/doctor.

The local backend writes the same file names and status directories used by `tickets.sh`:

- `work-items/{open,pending,closed}/<id>/item.md`
- `thread.md`
- `artifacts/`
- closed `resolution.md`

## 3. Requirement-by-requirement assessment

- Public/domain naming uses `Ticket`, not `WorkItem`: satisfied. Public names are `Ticket`, `TicketBackend`, `LocalTicketBackend`, `TicketStatus`, `TicketEvent`, etc.
- Crate/module placement: satisfied. `crates/ticket` is a low-level workspace crate and does not depend on `pod`, `tui`, or other high-level crates.
- Backend-oriented API shape: satisfied for this ticket. Local path concepts are mostly confined to `LocalTicketBackend`, `TicketArtifactRef`, and filesystem operations; core ticket/status/event metadata remains backend-neutral.
- Current layout compatibility: satisfied. The implementation preserves the status directories and expected markdown files, including `resolution.md` on close.
- Operation coverage: satisfied for this ticket. The backend covers list/show/create/add_event/review/set_status/close/doctor and includes diagnostics rather than hard failures for malformed existing records.
- Existing old/minimal frontmatter readability: satisfied. Parsing tolerates missing optional readiness/action-required/risk fields and unknown/extension metadata values.
- Markdown/freeform bodies: satisfied for `item.md` bodies and normal event/resolution bodies. See residual risk for the inherited `thread.md` delimiter limitation.
- Path containment/id safety/atomic-ish writes/locking: acceptable for a local-files MVP. IDs are validated as path components, writes are staged through temp files in the destination directory then renamed, and backend operations take an exclusive backend lock.
- Tests do not mutate real `work-items/`: satisfied. Tests use temp directories and pass an explicit `WORK_ITEMS_DIR` to `tickets.sh` compatibility checks.
- Tests exercise `tickets.sh` compatibility: satisfied. The tests create and mutate temp work item trees through both the Rust backend and `tickets.sh`, then run/show/doctor against the same layout.
- `Cargo.lock` / `package.nix`: acceptable. The new crate and `fs4` lock dependency require workspace/package metadata updates; `nix build .#yoi --no-link` passed.
- Scope control: satisfied. I did not find Pod tools, intake workflow routing, orchestrator scheduling, TUI UI, storage rename, or unrelated refactors in the implementation.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- `NewTicketEvent.references` / event references are modeled but not visibly persisted into or parsed from `thread.md`. That is acceptable for this backend MVP, but follow-up work should either define a compatible markdown representation or keep references as higher-level metadata outside the shell-compatible thread format.
- Extension/unknown enum variants are useful for reading older/future files, but local write paths should remain careful not to emit values that `tickets.sh doctor` would reject unless the file format is intentionally extended.
- The backend lock coordinates Rust backend callers, but it cannot coordinate concurrent direct `tickets.sh` writes. That is acceptable for this compatibility bridge, but users should not assume cross-tool transactional safety yet.

## 6. Validation assessed or rerun

Reviewed:

- Ticket item, delegation intent, parent Ticket definition/API shape, and `tickets.sh` compatibility reference.
- Diff against `develop...HEAD` for the implementation branch.
- New crate placement and workspace/package metadata.
- Local backend read/write, parser, status transition, close, doctor, and tests.

Reran from `/home/hare/Projects/yoi/.worktree/ticket-local-files-backend`:

- `cargo test -p ticket --no-run`
- `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check develop...HEAD`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- `nix build .#yoi --no-link`

All commands completed successfully.

## 7. Residual risk

The `thread.md` format still uses plain `---` event separators, so an event body containing a standalone `---` line remains ambiguous. This appears inherited from the current shell-compatible storage format rather than introduced by the new crate, and should be handled deliberately if future typed consumers require lossless arbitrary markdown event bodies.


---

<!-- event: implementation_report author: hare at: 2026-06-05T04:44:08Z -->

## Implementation report

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


---

<!-- event: close author: hare at: 2026-06-05T04:45:46Z status: closed -->

## Closed

Ticket local files backend is complete and merged.

Implementation:

- `740b017 feat: add local ticket backend`
- merge commit: `1041cdb merge: add local ticket backend`

Summary:

- Added a new low-level `ticket` workspace crate.
- Added typed Ticket domain types and `TicketBackend` trait.
- Added `LocalTicketBackend` over the current `work-items/` storage.
- Implemented list/show/create/add_event/review/set_status/close/doctor operations.
- Preserved current local storage layout and `tickets.sh` compatibility.
- Kept the implementation independent from `pod`, `tui`, Intake, Orchestrator routing, and scheduler code.
- Did not rename `work-items/` or remove `tickets.sh`.

Review:

- External sibling reviewer approved with no blockers.
- Non-blocker follow-ups recorded in the implementation report/review:
  - event references are modeled but not persisted/parsed in `thread.md` yet;
  - write paths should avoid emitting extension values that `tickets.sh doctor` rejects unless the format is intentionally extended;
  - backend lock coordinates Rust callers but not direct concurrent `tickets.sh` writes;
  - inherited `thread.md` `---` separator ambiguity remains.

Post-merge validation passed:

- `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- `nix build .#yoi --no-link`


---
