<!-- event: create author: "yoi ticket" at: 2026-07-10T15:53:02Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-10T15:54:24Z -->

## Decision

Worker / Session / workdir retention discussion from Objective 00001KWW44EXK:

- Worker is the archival unit; Session/transcript belongs under Worker archive.
- Worker retention must support pinned to protect selected history from cleanup/prune.
- Workdir files are cache when clean and reconstructable from repository + selector/resolved commit.
- Dirty workdir is an active/recovery state; dirty orphan should require recovery Worker or explicit discard, not automatic prune.
- Backend should keep canonical Worker/Workdir/link records in SQLite, while Runtime owns materialized files, raw paths, process/cwd binding, and cleanup execution.
- Runtime direct/unmanaged workdirs must be distinguishable from Backend-managed workdirs.


---

<!-- event: intake_summary author: hare at: 2026-07-10T16:06:07Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T16:06:07Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-10T16:10:57Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-10T16:11:45Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Backend SQLite Worker registry / Workdir registry / Worker-Workdir link table の scope、retention/link authority、Backend-managed vs unmanaged Runtime workdir distinction、acceptance criteria を具体化している。
- typed relation blocker は 0 件、OrchestrationPlan record は 0 件。queued Ticket はこの Ticket 1件のみ、inprogress は 0 件。
- Objective `00001KWW44EXK` の retention 方針に沿った concrete implementation Ticket であり、実装前に必要な missing decision は見当たらない。
- queued notification is human authorized routing/start。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX6BPY7M)`: 0 件。
- `TicketOrchestrationPlanQuery(00001KX6BPY7M)`: 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: this Ticket 1件。inprogress は 0 件。
- Recent code history includes Runtime workdir pages / Worker launch / Execution Workspace materialization / Browser-managed execution workspaces already merged, giving implementation substrate。

IntentPacket:

Intent:
- Add Backend-owned SQLite Worker registry, Workdir registry, and Worker-Workdir link authority so Backend can explain Worker/workdir retention and relationships independently of live Runtime catalog/path state。

Binding decisions / invariants:
- Worker is the archival/retention unit. Session/transcript refs belong under Worker archive/reference shape, not independent long-term authority。
- Workdir canonical record exists in Backend SQLite; Runtime owns actual files, raw materialized paths, process/cwd binding, cleanup execution。
- Worker-Workdir relation authority is a SQLite link table; summary copies are non-authoritative。
- Workdir files are reproducible cache, not durable history。
- Dirty workdir is active/recovery state; dirty orphan is not automatically pruned。
- Browser-facing UI uses `workdir` wording; internal `working_directory` compatibility names may remain during migration。
- Raw Runtime path must not be stored in Backend registry or shown in Browser responses。
- Runtime direct/unmanaged workdirs must be distinguishable from Backend-managed workdirs and not mixed into managed lists。

Requirements / acceptance criteria:
- Add typed Backend SQLite tables for Worker registry, Workdir registry, and Worker-Workdir links with migration/default path for existing workspaces。
- Backend workdir creation creates/updates Backend Workdir registry before requesting Runtime materialization。
- Backend Worker creation creates/updates Backend Worker registry and Worker-Workdir link。
- Runtime Worker / Workdir status observations sync into Backend registry without deleting canonical record when files disappear。
- Backend registry can answer stopped Worker -> removed/missing Workdir relationship。
- Worker archive/retention metadata can represent `pinned`。
- Browser Worker/Workdir lists are based on Backend-managed registry plus Runtime observation merge。
- Backend-managed vs unmanaged/external Runtime Workdirs are distinguishable。

Implementation latitude:
- Exact table names/enum names/API response shape/test helper layout can follow current `workspace-server` store/server style。
- Session/transcript refs can be future-compatible nullable/json-ish shape if full archive model is not yet implemented。
- Runtime observation sync can be minimal v0 as long as canonical Backend rows remain authoritative and unmanaged/external is typed。

Escalate if:
- Actual prune/delete execution, dirty orphan recovery Worker auto-start, session/transcript full deletion UI, or storing Runtime raw paths in Backend canonical records becomes necessary。

Validation:
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map:
- Backend store/schema: `crates/workspace-server/src/store.rs`。
- Backend server/workdir/worker APIs: `crates/workspace-server/src/server.rs`, `hosts.rs`, `companion.rs`。
- Runtime workdir/worker models: `crates/worker-runtime/src/{runtime.rs,working_directory.rs,execution_workspace.rs,catalog.rs,http_server.rs}`。
- Frontend workdir/worker pages: `web/workspace/src/routes/w/[workspaceId]/...`, workspace sidebar/workdir/worker helpers。

Critical risks / reviewer focus:
- Do not store or expose raw Runtime materialized paths。
- Backend registry/link table must remain relation authority, not transient Runtime observation。
- Runtime direct unmanaged workdirs must be visible/diagnosable as unmanaged, not silently treated as Backend-managed。
- Missing/removed workdir files must not erase canonical Backend records。
- `pinned` retention must be representable and protected from future cleanup/prune surfaces。

---

<!-- event: state_changed author: orchestrator at: 2026-07-10T16:12:07Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX6BPY7M)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KX6BPY7M)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket scope is Backend Worker/Workdir registry/link authority; actual prune/delete/recovery automation and raw Runtime path persistence are explicitly out of scope。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: hare at: 2026-07-10T16:38:10Z -->

## Implementation report

Implemented Backend SQLite Worker/Workdir registries and Worker-Workdir link authority.

Summary:
- Added schema v3 tables for worker_registry, workdir_registry, and worker_workdir_links with explicit retention_state including pinned.
- Added store records/APIs plus round-trip tests for pinned Worker retention, removed Workdir preservation, and links.
- Added Backend-managed Workdir id assignment and pending registry write before Runtime materialization.
- Updated Workspace API to serve Browser Workdir lists from backend-managed registry rows, preserve canonical Workdir records after cleanup/removal, and distinguish runtime_unmanaged records when Runtime-direct Workdirs are encountered.
- Registered created/listed Workers and linked Workers to selected Workdirs without storing raw Runtime materialized paths.
- Extended worker-runtime WorkingDirectoryRequest with an optional backend_workdir_id used by local worktree materialization.

Validation run:
- git diff --check
- cargo test -p yoi-workspace-server --lib
- cargo test -p worker-runtime --features ws-server,fs-store
- cargo check -p yoi
- cd web/workspace && deno task check && deno task test
- yoi ticket doctor
- nix build .#yoi --no-link


---
