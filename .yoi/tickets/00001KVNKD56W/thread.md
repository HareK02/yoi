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

<!-- event: review author: yoi-reviewer-00001KVNKD56W-r1 at: 2026-06-22T09:13:06Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket/design authority:
  - `.yoi/tickets/00001KVNKD56W/item.md`
  - `.yoi/tickets/00001KVNKD56W/thread.md`
  - `.yoi/tickets/00001KVNKD56W/artifacts/schema-v0.md`
- Implementation diff `f17a458a..5149ab70`:
  - `crates/workspace-server/src/store.rs`
  - `crates/workspace-server/src/server.rs`
  - `web/workspace/src/routes/+page.svelte`
  - schema artifact updates
- Target branch/head:
  - Branch: `impl/00001KVNKD56W-workspace-db-schema-v0`
  - Head: `5149ab703f63`

Blocking issue:

1. Legacy-upgraded `workspaces` table remains non-canonical and can reject new workspace inserts。

Fresh schema defines canonical `workspaces` as:
- `workspace_id`
- `display_name`
- `state`
- `created_at`
- `updated_at`

This matches `schema-v0.md`。

However, legacy migration does not rebuild or demote the old bootstrap `workspaces` table. It only adds `state`:
- `crates/workspace-server/src/store.rs:198-200`
  - `ALTER TABLE workspaces ADD COLUMN state TEXT NOT NULL DEFAULT 'active';`

Then `create_schema_v0_tables()` is called, but because it uses `CREATE TABLE IF NOT EXISTS workspaces`, the old table is left in place:
- `crates/workspace-server/src/store.rs:218-227`

The old v1 bootstrap `workspaces` table had extra `local_root TEXT NOT NULL` and `record_authority TEXT NOT NULL` columns. Those remain on upgraded DBs even though they are not part of canonical v0。

This is not merely cosmetic: `upsert_workspace()` now inserts only v0 columns:
- `crates/workspace-server/src/store.rs:87-89`

For an upgraded legacy DB, inserting a new `workspace_id` would fail on old `local_root` / `record_authority` NOT NULL columns because the insert no longer supplies them. Updating the already-existing legacy workspace may pass due conflict/update path, which is why current tests do not catch this。

Why this blocks:
- Upgraded canonical schema does not actually match `schema-v0.md`。
- Legacy handling is not safe enough because active canonical `workspaces` remains partly old schema and incompatible with the new write path。

Required fix:
- Rebuild/copy `workspaces` into canonical v0 shape, or otherwise explicitly preserve old workspace data without leaving obsolete NOT NULL columns on the active canonical table。
- Add tests asserting upgraded `workspaces` column set。
- Add a post-upgrade insert/upsert test for a new workspace id。

Non-blocking concerns:
- Existing tests cover fresh forbidden table absence and legacy demotion for old `runs` / projections / artifact / repository tables, but not upgraded `workspaces` column set or post-upgrade insert; add coverage with the fix。
- Runs card/reference removal is narrow and `/api/runs` removal is test-covered with 404 assertion。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check f17a458a..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed (`10 passed`)
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && rm -rf node_modules .svelte-kit build && deno task check && deno task build`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed
- `nix build .#yoi --no-link --print-out-paths`: passed/cached output `/nix/store/i0dr920x6kf217zddbnaqyljh8l1gwri-yoi-0.1.0`

Generated/ignored artifacts remain in reviewer worktree after validation:
- `target/`
- `web/workspace/node_modules/`
- `web/workspace/.svelte-kit/`
- `web/workspace/build/`

Worktree status:
- Tracked status clean; only ignored generated artifacts present。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-22T09:18:29Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVNKD56W`.

New fix commit:
- `d89ace5b workspace: canonicalize legacy workspaces`

Branch commits now:
- `5149ab70 workspace: implement db schema v0 bootstrap`
- `d89ace5b workspace: canonicalize legacy workspaces`

Fix summary:
- `crates/workspace-server/src/store.rs`
  - Legacy migration now preserves old non-canonical `workspaces` by renaming it to `legacy_workspaces`。
  - Active canonical v0 `workspaces` is created with only:
    - `workspace_id`
    - `display_name`
    - `state`
    - `created_at`
    - `updated_at`
  - Existing legacy workspace rows are copied into canonical `workspaces`, defaulting `state` to `active` when no typed state existed。
  - Previous legacy demotion for `repositories`, `runs`, `artifacts`, `ticket_projections`, and `objective_projections` is preserved。
  - Added upgrade test assertions for:
    - upgraded `workspaces` column set exactly matches v0。
    - `legacy_workspaces` preserves old obsolete columns。
    - old workspace data is copied into canonical table。
    - post-upgrade `upsert_workspace()` succeeds for a new workspace id。
- `.yoi/tickets/00001KVNKD56W/artifacts/schema-v0.md`
  - Updated implementation alignment notes to document `legacy_workspaces` preservation and canonical copy behavior。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `git diff --check f17a458a..HEAD`: passed
- `cargo run -p yoi -- ticket doctor`: passed

Not rerun by Coder for the r1 fix:
- Deno check/build and Nix build, because the fix touched only schema artifact and workspace-server store code and Reviewer had validated frontend/Nix before the blocker。

Generated artifact cleanup:
- Coder removed reviewer-generated frontend artifacts:
  - `web/workspace/node_modules/`
  - `web/workspace/.svelte-kit/`
  - `web/workspace/build/`

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVNKD56W-workspace-db-schema-v0` at `d89ace5b`。
- r1-fix diff `5149ab70..HEAD` touches only `crates/workspace-server/src/store.rs` and `schema-v0.md`。
- `git diff --check f17a458a..HEAD`: passed。
- No ignored frontend artifacts remain under `web/workspace`。

Next action:
- Route back to Reviewer for r2 verification of legacy `workspaces` canonicalization and final acceptance。

---

<!-- event: review author: yoi-reviewer-00001KVNKD56W-r1 at: 2026-06-22T09:24:03Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- r1-fix diff `5149ab70..d89ace5b`:
  - `crates/workspace-server/src/store.rs`
  - `.yoi/tickets/00001KVNKD56W/artifacts/schema-v0.md`
- Overall diff `f17a458a..d89ace5b`:
  - `.yoi/tickets/00001KVNKD56W/artifacts/schema-v0.md`
  - `crates/workspace-server/src/server.rs`
  - `crates/workspace-server/src/store.rs`
  - `web/workspace/src/routes/+page.svelte`

Previous blocker verification:
- Resolved。

Evidence:
- Non-canonical legacy `workspaces` is now detected by exact v0 column comparison and renamed to `legacy_workspaces`。
- Canonical v0 `workspaces` is then created with exactly:
  - `workspace_id`
  - `display_name`
  - `state`
  - `created_at`
  - `updated_at`
- Legacy workspace rows are copied into canonical `workspaces`, with `state` defaulted to `active` when absent。
- Tests assert:
  - upgraded `workspaces` columns exactly match v0。
  - `legacy_workspaces` preserves old obsolete columns。
  - legacy workspace data is readable through canonical `get_workspace`。
  - post-upgrade `upsert_workspace()` succeeds for a new workspace id。

Blocking issues:
- None。

Non-blocking concerns:
- None requiring changes。

Ticket/design fit:
- Fresh schema creates typed v0 tables and does not create canonical top-level `runs`, `hosts`, `workers`, `actors`, or validation/CI result tables。
- Legacy `runs` is preserved only as `legacy_runs`; it does not masquerade as canonical。
- Host/Worker APIs remain live runtime views, not DB authority。
- `/api/runs` remains removed/404-tested。
- Canonical schema avoids generic `metadata_json`, `payload_json`, `diagnostics_json`, and catch-all payload columns。
- `schema-v0.md` update is narrow and consistent with migration behavior。
- No accidental frontend authority/business-logic expansion found。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check f17a458a..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed (`10 passed`)
- `cargo check -p yoi-workspace-server`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link --print-out-paths`: passed, output `/nix/store/fnz7650sshl3md2vdwb4a1q8dc0hf542-yoi-0.1.0`

Not rerun in r2:
- Deno frontend check/build, because r1-fix diff touches only `store.rs` and `schema-v0.md`; r1 had already validated frontend for original `/api/runs` card removal。

Worktree status:
- Clean tracked status on `impl/00001KVNKD56W-workspace-db-schema-v0` at `d89ace5b`。
- Ignored artifact present: `target/` only; no ignored frontend artifacts under `web/workspace`。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-22T09:25:49Z -->

## Implementation report

Reviewer r2 approvalを受けて、実装ブランチを Orchestrator worktree の `orchestration` branch に統合した。

統合:
- Merge commit: `38bd122d merge: workspace db schema v0`
- Merged branch: `impl/00001KVNKD56W-workspace-db-schema-v0`
- Implementation commits:
  - `5149ab70 workspace: implement db schema v0 bootstrap`
  - `d89ace5b workspace: canonicalize legacy workspaces`

Orchestrator worktree validation:
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed (`10 passed`)
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && rm -rf node_modules .svelte-kit build && deno task check`: passed (`svelte-check found 0 errors and 0 warnings`)
- `cd web/workspace && deno task build`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed

Validation generated ignored frontend artifacts in Orchestrator worktree:
- `web/workspace/node_modules/`
- `web/workspace/.svelte-kit/`
- `web/workspace/build/`

These were removed after validation. Final Orchestrator worktree status after validation cleanup is clean on `orchestration` at `38bd122d`。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-22T09:26:02Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Reviewer approval、Orchestrator worktree への統合、workspace-server tests/check、Deno check/build、Ticket doctor、Nix build が完了したため `done` に遷移する。

---

<!-- event: state_changed author: hare at: 2026-06-22T09:26:19Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-22T09:26:19Z status: closed -->

## 完了

Workspace DB canonical schema v0 design を `schema-v0.md` artifact と workspace-server SQLite bootstrap/migrations に整合させ、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- SQLite schema version を `2` に進め、canonical schema v0 を実装。
- Typed v0 tables を追加:
  - `tickets`
  - `ticket_events`
  - `ticket_relations`
  - `objectives`
  - `objective_ticket_links`
  - `repositories`
  - `ticket_targets`
  - `ticket_target_paths`
  - `ticket_worker_links`
  - `artifacts`
  - `audit_events`
- Canonical v0 から top-level `runs`, `hosts`, `workers`, `actors`, validation/CI result tables を除外。
- Generic `metadata_json`, `payload_json`, `diagnostics_json` のような catch-all payload columns を canonical v0 tables に追加しない方針を維持。
- `/api/runs` と frontend Runs card/reference を削除し、404 test を追加。
- Host/Worker APIs は DB authority ではなく live runtime views として維持。
- Legacy bootstrap tables は non-canonical `legacy_*` preservation tables に demote。
- Legacy `workspaces` は `legacy_workspaces` に preserve し、active canonical `workspaces` を v0 column set で作り直して既存行を copy。
- Post-upgrade `upsert_workspace()` for new workspace id が通る regression test を追加。
- `schema-v0.md` に SQLite version 2 / legacy preservation alignment notes を追加。

統合・検証:
- Merge commit: `38bd122d merge: workspace db schema v0`
- Implementation commits: `5149ab70`, `d89ace5b`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi-workspace-server`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Ticket/Objective write authority migration to DB is not implemented。
- Host/Worker canonical DB tables are not added。
- Validation/CI result tables and Actor table are not added。
- Full TicketEvents/TicketWorkerLinks/Artifacts write surfaces are not implemented beyond schema/bootstrap alignment。

---
