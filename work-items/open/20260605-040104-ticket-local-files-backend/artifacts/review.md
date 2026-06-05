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
