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

<!-- event: implementation_report author: orchestrator at: 2026-07-10T16:13:15Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KX6BPY7M-worker-workdir-registry`
  - Branch: `work/00001KX6BPY7M-worker-workdir-registry`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KX6BPY7M-worker-workdir` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- Coder には短い task と Ticket record 読み取り指示を渡した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T16:40:17Z -->

## Implementation report

Implementation progress report:

- Coder Pod completed implementation and committed `3e5546ea feat: add backend worker workdir registry` on branch `work/00001KX6BPY7M-worker-workdir-registry`。
- Worktree status was clean after commit。
- Changed files include:
  - `crates/workspace-server/src/store.rs`
  - `crates/workspace-server/src/server.rs`
  - `crates/worker-runtime/src/{catalog.rs,worker_backend.rs,working_directory.rs}`
  - child implementation also committed `.yoi/tickets/00001KX6BPY7M/{item.md,thread.md}` changes; Orchestrator will preserve authoritative Ticket records during merge if conflicts occur。
- Implementation summary:
  - Added Backend SQLite schema v3 tables: `worker_registry`, `workdir_registry`, `worker_workdir_links`。
  - Added typed store records/APIs for Worker archival registry, Workdir registry, Worker-Workdir link authority, and explicit `pinned` retention representation。
  - Backend-managed Workdir IDs and pending registry writes are created before Runtime materialization。
  - Browser Workdir lists are based on Backend-managed registry rows。
  - Runtime-direct Workdirs are distinguished as `runtime_unmanaged`。
  - Canonical Workdir records are preserved after cleanup/removal。
  - Worker creation/listing records Backend Worker registry entries and links selected Workdirs without storing raw Runtime materialized paths。
  - Extended worker-runtime `WorkingDirectoryRequest` with optional `backend_workdir_id`。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p yoi-workspace-server --lib`: pass（95 tests）
  - `cargo test -p worker-runtime --features ws-server,fs-store`: pass（48 lib tests + 5 main tests + doc tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（23 tests）
  - `yoi ticket doctor`: ok
  - `nix build .#yoi --no-link`: pass
- External review will be requested via a read-only sibling Reviewer Pod. `StopPod` は使わない。

---

<!-- event: review author: reviewer at: 2026-07-10T16:52:25Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blockers:

1. Browser Worker list が Backend registry based になっていない。
   - Ticket は Worker/Workdir lists を Backend-managed registry + Runtime observation merge に基づかせることを要求している。
   - `workers_response` はまだ `api.runtime.list_workers(limit)` を呼び、live Runtime items を opportunistic に記録したうえで `items: runtime_workers.items` を返している（`crates/workspace-server/src/server.rs:2529-2546`）。
   - `worker_registry` / `worker_workdir_links` を read していないため、Backend-only / stopped / archive rows を list できず、relation authority が Browser Worker list に使われていない。

2. Runtime Worker creation path が Worker registry / Workdir registry / links を書いていない。
   - `create_runtime_worker` は `working_directory_request` を解決して `api.runtime.spawn_worker` に forward し、Runtime result を返すだけ（`server.rs:2269-2285`）。
   - Runtime materialization 前の pending Backend Workdir row、`record_worker_summary`、Worker-Workdir link がない。
   - Browser `/api/workers` path のみ existing Workdir を record/link-select している（`server.rs:2141-2170`）。
   - Backend-created/internal Runtime Workers with materialized workdirs が untracked になり、「Backend worker creation writes/updates Worker registry and Worker-Workdir link」 acceptance に違反する。

3. Stopped Worker -> removed/missing Workdir relationship が Backend registry から安定して答えられない。
   - `stop_runtime_worker` は Runtime stop を forward して返すだけ（`server.rs:2330-2339`）。
   - Runtime cleanup 後に Worker lifecycle、Workdir materialization status、link state を Backend SQLite に更新しない。
   - `working_directory_summaries` は `list_managed_workdir_registry` だけを読む（`server.rs:3242-3249`）ため、Runtime stop が bound workdir を remove しても Backend record は stale `present` のままになり得る。

4. Runtime Workdir observation / unmanaged distinction が partial。
   - runtime-scoped workdir list は raw Runtime summaries を直接返す（`server.rs:1184-1200`）。
   - workspace-scoped list は Backend-managed rows のみ返す。
   - `runtime_unmanaged` rows は detail / worker-create paths で opportunistic に作られるだけで、list 時に sync/typed されない。
   - Browser summaries も management kind を露出していないため、unmanaged/external Runtime Workdirs を Backend-managed Workdirs と区別して visible/diagnosable にする requirement を満たしていない。

5. pinned retention が sync で clobber され得る。
   - schema は `pinned` を保存できるが、`record_worker_summary` は常に `retention_state: "normal"` を書く（`server.rs:3290-3318`）。
   - `upsert_worker_registry` は conflict 時に `retention_state = excluded.retention_state` で上書きする（`store.rs:223-242`）。
   - pinned Worker への後続 Runtime observation / list sync が pinned を normal に downgrade できるため、`pinned` が durable retention metadata として保護されていない。

Non-blocking follow-ups:
- schema 方向性自体は妥当。`worker_registry`, `workdir_registry`, `worker_workdir_links` は typed checks / FK relations 付きで存在する（`store.rs:488-545`）。
- Backend workdir creation が Runtime materialization 前に pending Backend row を作る点は良い（`server.rs:1276-1327`）。
- inspected paths では raw materialized Runtime paths が新 Backend registry records に保存されている箇所は見当たらなかった。
- tests は schema roundtrip / pinned storage / removed Workdir / link basics を cover しているが、上記 blocker paths（runtime worker spawn with workdir, stop cleanup syncing, Backend-registry Worker list projection, unmanaged list diagnostics, pinned preservation under sync）は不足。

Validation performed:
- Ticket item/thread including routing decision, IntentPacket, implementation report を確認。
- implementation commit `3e5546ea` の `git show --name-only` / `git show --check` を確認。
- implementation worktree 内だけで branch/status を確認。
- `store.rs`, `server.rs`, `catalog.rs`, `worker_backend.rs`, `working_directory.rs` を review。
- acceptance blockers が inspection で明確だったため、full test/build matrix は再実行していない。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T17:14:02Z -->

## Implementation report

Follow-up implementation progress report after request_changes:

- Coder Pod `yoi-coder-00001KX6BPY7M-worker-workdir` completed the focused fix pass and committed follow-up commit `8695089fba346b8f10d55016b8328337e1c37445 fix: sync backend worker registry projections` on branch `work/00001KX6BPY7M-worker-workdir-registry`。
- Implementation worktree status was clean at Orchestrator inspection。
- Orchestrator inspected branch log / commit stats / `git diff --check 86f652a7..HEAD` in the implementation worktree。

Coder-reported fix summary:
- Browser Worker list now projects from Backend `worker_registry` rows after Runtime observation sync, preserving Backend-only stopped/archive rows。
- Worker-Workdir relationships are projected from `worker_workdir_links` as authority。
- Runtime-scoped Worker spawn now writes pending Backend-managed Workdir rows, Worker registry rows, and Worker-Workdir links。
- Worker stop now updates Backend Worker lifecycle and syncs linked Workdir status to present/removed/missing without deleting canonical rows。
- Runtime Workdir observation/listing now syncs Runtime observations into Backend registry and projects safe `management_kind` (`backend_managed` / `runtime_unmanaged`)。
- Existing `pinned` Worker retention is protected from ordinary Runtime sync/upsert clobbering。
- Tests were added/strengthened for archive/link projection, unmanaged Workdir projection, raw path redaction, removed Workdir preservation, and pinned retention preservation。
- Frontend type exposure for Workdir `management_kind` was added。

Coder-reported validation passed:
- `git diff --check`
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Next action:
- Request focused external re-review against commits `3e5546ea..8695089f` and the whole Ticket acceptance criteria before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-10T17:22:14Z status: request_changes -->

## Review: request changes

Focused re-review result: request_changes

Evidence reviewed:
- Implementation worktree is on `work/00001KX6BPY7M-worker-workdir-registry` at `8695089fba346b8f10d55016b8328337e1c37445`, clean。
- Follow-up fixes most prior blockers:
  - Worker list now syncs Runtime observations, then projects from `worker_registry` / `worker_workdir_links` (`workers_response`, `merge_worker_registry_projection`)。
  - Runtime Worker create now writes pending Backend Workdir rows, records Worker rows, and links selected/materialized Workdirs。
  - Stop path now updates Worker lifecycle and attempts linked Workdir status sync。
  - Store upsert preserves existing `pinned` when a Runtime sync supplies `normal`。
  - New registry records still do not store raw materialized Runtime paths。

Blocker:
- Unmanaged Runtime Workdirs are now mixed into normal Backend-managed Browser lists / launch options。
  - Ticket requires Runtime-direct/unmanaged Workdirs to be distinguishable and “not mixed into managed lists” (`item.md:61`)。
  - Follow-up syncs unknown Runtime Workdirs as `runtime_unmanaged`, but `working_directory_summaries()` returns all `workdir_registry` rows via `list_workdir_registry()` (`server.rs:3300-3308`)。
  - This feeds Browser launch options (`server.rs:3278`) and the workspace-scoped Workdirs list。
  - The worker-new UI select iterates `options.working_directories` without filtering or labeling `management_kind`, so a `runtime_unmanaged` Workdir can appear/select like a normal managed Workdir。
  - This is not a safe typed UI distinction and does not satisfy the Backend-managed vs unmanaged boundary。

Non-blocking notes:
- Runtime-scoped Workdir lists being typed from registry is a good direction。
- Consider ensuring detail/cleanup/create responses also project through `workdir_summary_from_record`; several paths still return Runtime summaries with `management_kind: None`, even after storing a typed Backend record。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T17:32:02Z -->

## Implementation report

Second follow-up implementation progress report:

- Coder Pod `yoi-coder-00001KX6BPY7M-worker-workdir` completed the focused unmanaged-workdir fix and committed `239f93b084fdcc2201e17fcdbc23eb0140531f16 fix: keep unmanaged workdirs out of managed lists` on branch `work/00001KX6BPY7M-worker-workdir-registry`。
- Implementation worktree status was clean at Orchestrator inspection。
- Orchestrator inspected branch log / commit stats / `git diff --check 86f652a7..HEAD` in the implementation worktree。

Coder-reported fix summary:
- Workspace-scoped Browser Workdir summaries / launch options now include only `backend_managed` Workdir registry rows。
- Runtime-direct/unmanaged Workdirs remain visible through runtime-scoped projection with explicit `management_kind: runtime_unmanaged`。
- Tests prove:
  - `runtime_unmanaged` rows are excluded from managed Browser Workdir summaries / launch options。
  - unmanaged rows remain distinguishable in runtime projection。
  - store-level managed listing excludes unmanaged rows while all-row listing still includes them。

Coder-reported validation passed:
- `git diff --check`
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Next action:
- Request focused re-review on remaining unmanaged-list blocker and full acceptance status before merge/close decisions。

---
