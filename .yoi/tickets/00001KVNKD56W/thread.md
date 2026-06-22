<!-- event: create author: "yoi ticket" at: 2026-06-21T17:24:43Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-21T17:25:35Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T17:25:35Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-22T08:48:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-22T08:49:46Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket includes a concrete `schema-v0.md` artifact defining the Workspace DB canonical schema target and migration stance。
- Artifact is precise enough to implement a non-breaking schema/migration alignment slice and/or finish design documentation without inventing semantics。
- Relations are non-blocking context relations; no blocker relation is recorded。
- Current Workspace backend has an older bootstrap SQLite schema in `crates/workspace-server/src/store.rs` with `runs`, `ticket_projections`, `objective_projections`, and run-linked artifacts, while `schema-v0.md` explicitly says no separate `Run` entity and Host/Worker are live views. This gives a concrete implementation alignment target。
- Orchestrator worktree is clean on `orchestration` at `b7c890d3`; target worktree / branch is not present。
- Current queued Ticket is this Ticket only。

Evidence checked:
- Ticket body / thread / artifacts via `TicketShow` and direct read of `artifacts/schema-v0.md`。
- `TicketRelationQuery(00001KVNKD56W)`: 2 relation records, no blocking relation reported。
- `TicketOrchestrationPlanQuery(00001KVNKD56W)`: no records。
- `TicketList(state=queued)`: this Ticket is the only queued Ticket。
- Orchestrator git state / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。
- Bounded code map:
  - `crates/workspace-server/src/store.rs` currently owns SQLite migrations and schema versioning。
  - Current store still has bootstrap `runs` and projection-oriented tables; `schema-v0.md` says v0 should not include separate `runs`, Host/Worker canonical DB tables, generic JSON blobs, or validation result tables。
  - Current repository/objective/ticket read APIs are filesystem read-through and must keep working。

IntentPacket:

Intent:
- Turn the Workspace DB canonical schema v0 design into a durable implementation/design artifact and align the current SQLite bootstrap migration where safely non-breaking。

Binding decisions / invariants:
- Ticket and Objective filesystem records remain the active authority; do not switch write authority to DB。
- No generic JSON/payload/metadata columns in the v0 canonical schema。
- Do not add canonical `hosts` / `workers` DB tables; Host/Worker remain live runtime views。
- Do not add a top-level `runs` canonical table; execution history belongs to Ticket events, TicketWorkerLinks, and Artifacts。
- Do not add CI/validation result tables in this Ticket。
- Do not add Actor table in v0; authorship is embedded typed snapshot fields。
- Keep existing Workspace APIs/tests working。
- Non-breaking migration is acceptable; destructive migration of existing user DBs is not required in this slice unless clearly safe and tested。

Requirements / acceptance criteria:
- `schema-v0.md` remains consistent with implemented/bootstrap migration semantics。
- Workspace-server migration/schema tests cover new v0 schema version and expected tables/absence of forbidden tables。
- Empty/safe tables from minimal implementation guidance may be added: `repositories`, `ticket_targets`, `ticket_target_paths`, `ticket_worker_links`, `artifacts`, `audit_events`, and supporting Ticket/Objective current/event tables if included。
- Remove or replace obsolete `runs` table/API/store concept if it conflicts with v0, or clearly demote it to non-canonical compatibility only if removal is out of scope。
- No canonical host/worker tables are added。
- Existing filesystem read-through APIs for Tickets/Objectives/Repository pages keep passing。
- Docs/tests make clear that Host/Worker API is live view and Ticket thread/events are execution authority。

Implementation latitude:
- If full schema migration is too large, implement the smallest safe migration alignment plus tests and record remaining tables as future follow-up comments。
- Prefer typed tables/columns over catch-all text/json columns。
- SQLite schema version can advance if migrations change。
- `runs` API placeholder may be removed if no longer used; ensure frontend/tests do not depend on it。
- Keep API surface read-only unless Ticket explicitly requires otherwise; this one does not require write APIs。

Escalate if:
- Removing current `runs` table/API would break existing accepted frontend or tests in a way that needs product decision。
- Existing DB migration compatibility requires a complex destructive migration or data backfill policy。
- Implementing full TicketEvents/TicketWorkerLinks/Artifacts read/write surfaces grows beyond schema/bootstrap alignment。
- Schema design conflicts with recent Repository/Objectives pages in a way not anticipated by artifact。

Validation plan:
- `cargo fmt --check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi-workspace-server`
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link` if package/source/dependency behavior changes。
- Deno check/build only if frontend/runs API removal touches frontend。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-22T08:49:57Z from: queued to: inprogress reason: human_authorized_schema_design_alignment field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has a concrete schema-v0 design artifact and current workspace-server migration has a clear non-breaking alignment target, so Orchestrator accepts implementation/design alignment.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-22T08:51:00Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVNKD56W-workspace-db-schema-v0`
- Created branch:
  - `impl/00001KVNKD56W-workspace-db-schema-v0`
- Base commit:
  - `f17a458a ticket: accept workspace db schema v0`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVNKD56W`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVNKD56W-workspace-db-schema-v0`

Next action:
- Wait for Coder implementation/design-alignment report, then spawn Reviewer read-only for the implementation diff。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-22T09:02:51Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVNKD56W`.

Implementation commit:
- `5149ab70 workspace: implement db schema v0 bootstrap`

Changed areas:
- `.yoi/tickets/00001KVNKD56W/artifacts/schema-v0.md`
  - Added implementation alignment notes for SQLite schema version 2。
  - Documented legacy preservation/demotion for old bootstrap tables。
- `crates/workspace-server/src/store.rs`
  - Replaced old bootstrap schema with Workspace DB canonical schema v0 tables。
  - Advanced schema version to `2`。
  - Added typed v0 tables: `tickets`, `ticket_events`, `ticket_relations`, `objectives`, `objective_ticket_links`, `repositories`, `ticket_targets`, `ticket_target_paths`, `ticket_worker_links`, `artifacts`, `audit_events`。
  - Removed active/canonical `runs` store concept and `RunSummary`。
  - Added legacy migration handling that renames old bootstrap tables to `legacy_*` preservation tables instead of treating them as canonical。
  - Added tests for fresh v0 schema boundaries and legacy upgrade behavior, including forbidden table absence。
- `crates/workspace-server/src/server.rs`
  - Removed `/api/runs` route。
  - Kept Host/Worker APIs as live runtime views。
  - Added test coverage that `/api/runs` is no longer served。
- `web/workspace/src/routes/+page.svelte`
  - Removed Runs endpoint card/reference。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `git diff --check`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `cd web/workspace && deno task check`: passed
- `cd web/workspace && deno task build`: passed

Not run by Coder:
- `nix build .#yoi --no-link`: not run because no dependency/package/source-filter changes。

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVNKD56W-workspace-db-schema-v0` at `5149ab70`。
- Diff `f17a458a..HEAD` touches expected design/backend/frontend files only。
- `git diff --check f17a458a..HEAD`: passed。
- Deno validation generated ignored frontend artifacts; Orchestrator removed `web/workspace/node_modules/`, `.svelte-kit/`, and `build/` before Reviewer routing。

Next action:
- Route to read-only Reviewer Pod for acceptance review。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-22T09:03:49Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVNKD56W-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVNKD56W-workspace-db-schema-v0`
- Branch: `impl/00001KVNKD56W-workspace-db-schema-v0`
- Baseline: `f17a458a`
- Implementation commit: `5149ab70 workspace: implement db schema v0 bootstrap`
- Diff: `f17a458a..5149ab70`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Consistency between `schema-v0.md` and implemented SQLite migration/version 2。
- Absence of canonical `runs`, `hosts`, `workers`, `actors`, generic JSON/payload/metadata columns, and validation/CI result tables。
- Legacy table preservation/demotion behavior。
- Existing read APIs for Tickets/Objectives/Repositories/Hosts/Workers still work。
- `/api/runs` removal is intentional and test-covered。
- Design artifact updates remain concise and consistent。

Orchestrator will wait for reviewer verdict before integration。

---
