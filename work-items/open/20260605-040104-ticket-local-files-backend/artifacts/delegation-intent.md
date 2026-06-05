# Delegation intent: Ticket local files backend

## Classification

`implementation-ready` with a constrained design boundary.

The Ticket concept, split, and terminology have been accepted in the parent umbrella. This ticket should implement the first layer only: a typed Rust Ticket domain/backend and a LocalTicketBackend over the current `work-items/` files.

## Intent

Add a code-facing Ticket domain model and local files backend so yoi can read and mutate the current repository Ticket records without shelling out to `tickets.sh` or ad hoc parsing.

The durable orchestration concept is named `Ticket`. The current local storage directory remains `work-items/` for compatibility.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/ticket-local-files-backend`
- branch: `work/ticket-local-files-backend`

## Requirements

- Add a new backend-shaped Rust layer for Tickets.
  - Use `Ticket`, not `WorkItem`, as the public/domain concept name.
  - Prefer a new lower-level workspace crate such as `crates/ticket` unless current code mapping reveals a better fit.
  - Keep this crate independent from `pod` and `tui`.
- Implement `LocalTicketBackend` over current `work-items/` storage.
- Preserve compatibility with `tickets.sh` and existing `work-items/{open,pending,closed}/<id>/` layout.
- Preserve these local files:
  - `item.md` with YAML-ish frontmatter and Markdown body;
  - `thread.md` append-only event-ish log;
  - `artifacts/` directory;
  - `resolution.md` for closed tickets.
- Implement typed operations equivalent to:
  - list;
  - show;
  - create;
  - add event/comment for roles `comment`, `plan`, `decision`, `implementation_report`;
  - review approve/request-changes;
  - status transition across open/pending/closed;
  - close with resolution;
  - doctor/consistency check.
- Provide a thin typed envelope around Markdown/freeform content.
- Include fields useful for later Orchestrator/TUI/tool layers where practical:
  - id / slug / title;
  - status;
  - kind / priority / labels;
  - created_at / updated_at;
  - readiness / needs_preflight / risk flags / action_required if present, while tolerating old tickets without them;
  - event kind / review result.
- Keep unknown/extension values parseable rather than failing unnecessarily.
- Implement safe local mutation:
  - path containment under configured root;
  - atomic replace for file rewrites where practical;
  - basic backend lock/conflict handling for concurrent Pod/tool callers;
  - no writes outside the configured local ticket root except controlled temp files.
- Add focused tests using temp directories/fixtures; do not mutate real `work-items/` in tests.
- Keep `tickets.sh` command behavior intact.

## Non-goals

- Pod tools / built-in feature integration.
- Intake workflow/profile implementation.
- Orchestrator routing implementation.
- TUI UI changes.
- Renaming `work-items/` to `tickets/`.
- Removing or replacing `tickets.sh`.
- External tracker backends.
- Scheduler/lease/automatic maintainer behavior.
- Changing the session-local Task tool.

## Current code map

- `tickets.sh`
  - Current authoritative mechanical behavior for local ticket creation/list/show/comment/review/status/close/doctor.
  - Use it as compatibility reference, not as a runtime dependency for the Rust backend.
- `work-items/{open,pending,closed}/<id>/item.md`
  - Ticket frontmatter and Markdown body.
  - Required existing fields: `id`, `slug`, `title`, `status`, `kind`, `priority`, `labels`, `created_at`, `updated_at`, `assignee`, `legacy_ticket`.
- `work-items/{open,pending,closed}/<id>/thread.md`
  - Current event log format uses HTML comments like `<!-- event: ... author: ... at: ... -->`, heading/body, and `---` separators.
- `work-items/{open,pending,closed}/<id>/artifacts/`
  - Controlled artifact directory for a ticket.
- `work-items/closed/<id>/resolution.md`
  - Resolution body for closed tickets.
- root `Cargo.toml`
  - Add the new crate to workspace members.
- `work-items/open/20260601-031252-builtin-work-item-intake-routing/artifacts/ticket-definition-and-api-shape-20260605.md`
  - Current conceptual Ticket definition/API direction.

## Suggested API shape

The exact API may be adjusted, but keep the layer backend-shaped:

```rust
trait TicketBackend {
    fn list(&self, filter: TicketFilter) -> Result<Vec<TicketSummary>>;
    fn show(&self, id: TicketIdOrSlug) -> Result<Ticket>;
    fn create(&self, input: NewTicket) -> Result<TicketRef>;
    fn add_event(&self, id: TicketIdOrSlug, event: NewTicketEvent) -> Result<()>;
    fn review(&self, id: TicketIdOrSlug, review: TicketReview) -> Result<()>;
    fn set_status(&self, id: TicketIdOrSlug, status: TicketStatus) -> Result<()>;
    fn close(&self, id: TicketIdOrSlug, resolution: MarkdownText) -> Result<()>;
    fn doctor(&self) -> Result<TicketDoctorReport>;
}
```

It is acceptable to expose concrete `LocalTicketBackend` methods first if trait object design becomes premature, but the code should not bake local paths into the Ticket concept types.

## Compatibility details to preserve

- `tickets.sh create` id format is `<YYYYMMDD-HHMMSS>-<slug>`.
- Status directories are `open`, `pending`, `closed`.
- `status` frontmatter must match containing directory.
- `doctor` checks required fields, id/directory match, duplicate ids/slugs, required files/directories, and legacy `tickets/*.md` references.
- `close` moves the ticket to `closed`, writes `resolution.md`, and appends a close event.
- `review` appends a review event with `status: approve` or `request_changes` in the event comment.

## Escalate if

- Preserving compatibility requires changing existing `tickets.sh` behavior.
- A strict typed schema would reject many existing tickets.
- The lock/atomic write approach requires a new dependency with license/packaging implications.
- The new crate needs to depend on `pod`, `tui`, or other high-level crates.
- Implementing all operations in one commit becomes too large; prefer a clean subset and report what remains.

## Validation

Run at least:

- new crate unit tests;
- compatibility tests that create/comment/review/status/close in temp dirs and verify `doctor` behavior;
- `cargo test -p ticket` or the chosen crate package name;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `./tickets.sh doctor`.

If feasible, run `nix build .#yoi` after the workspace check passes.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- chosen crate/module name;
- public API summary;
- changed files;
- compatibility with `tickets.sh`;
- tests/validation run;
- unresolved risks/follow-ups;
- whether ready for external review.
