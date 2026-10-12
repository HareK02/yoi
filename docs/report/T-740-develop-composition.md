# T-740: composition with develop 71ab4c06

## Source reconciliation

The existing `work/T-740-workspace-ticket-tags` source at `e9397fa0` was clean. The requested develop commit `71ab4c06b9ae24ca4daa88516d1e436c5a31c779` was fetched and merged normally into this same branch (no replacement MR or integration branch). Generated conflicts in OpenAPI and companion/repository/runtime/ticket/worker-launch types were resolved by regeneration, not by selecting one side's contract. Workdir types were regenerated as well because the shared contract digest changed.

The merged Rust contract retains both classification operations/projections and the develop Runtime binding/per-request authentication model. Workspace schema remains 90; Ticket classification schema remains 8. The Runtime migration archives historical verification metadata, does not make it a grant, and does not rebuild Ticket/tag tables. Shared server/store/schema-equivalence/purge inventory changes were inspected against develop: both Runtime archives and all three classification tables are included.

Official generators rerun:

- canonical `export_openapi`;
- repository/runtime/ticket/workdir/worker-launch/companion OpenAPI schema-closure generators;
- feature-gated memory/skill/auth/repository-access/Drive/legacy Server type catalogs. Their checked-in representation conventions are preserved (auth uses the raw normalized generator output, not Deno reformatting).

## Composition defect fixed

New signed-Runtime router tests found that classification handlers discarded the verified Runtime Worker identity when optional transport source-hint headers were absent. This produced empty attribution in durable tag receipts and allowed another authenticated Worker to replay the same operation key.

Classification now takes the verified Runtime source from `ServerRequestContext`, retains the existing source-hint verification path, rejects mismatched identities, and carries the actual Worker identity into event/audit and exact-replay input. Runtime-only claims without a Worker identity cannot impersonate a human classification actor. Request proof tokens are not operation identity; exact replay with a new valid signature from the same Worker is accepted, while another Worker conflicts. Other Ticket operation contracts are not respecified by this correction.

## Executed validation

All fixtures are hermetic, temporary, in-process boundaries following `docs/development/rust-testing-strategy.md`.

- `cargo test -p yoi-workspace-server --lib ticket_tags_tests`: 6 passed (original 3 plus signed Runtime lifecycle/replay, identity-bound replay, and rejection matrix).
- Rejection coverage: unsigned hints, forged signing key, unknown Worker, other Workspace claim, body tampering, stale pinned key, and administratively revoked binding, for both catalog mutation and Ticket membership endpoints; no rejected-operation writes or execution effects.
- `current_workspace90_without_classification_upgrades_ticket7_without_rewriting_ticket_history`: passed. A current Workspace-90/pre-classification Ticket-7 fixture upgrades to Ticket-8 without rewriting an existing closed Ticket, labels, decision/history, or Workspace auth schema; arbitrary `ready` classification preserves the decision.
- `workspace89_to90_runtime_rebuild_preserves_tag_links_tombstones_receipts_and_audit`: passed. Real retained migration plus tagged data preserves exact current links, tombstones/recreated identities, receipt/context/audit rows and thread snapshots; replay and FK/deleted-reference rejection remain intact.
- `fresh_and_upgraded_state_contract_schemas_enforce_the_same_constraints`: passed, including classification tables and Runtime archives.
- `cargo test -p ticket --lib tags`: 14 passed.
- `cargo test -p worker --lib feature::builtin::ticket::tags`: 7 passed, covering normal tools/native WIP, permission, route binding and durable mutation/replay contracts after composition.
- `cargo test -p worker-runtime --lib workspace_issuer`: 12 passed.
- Full `cargo test -p ticket -p server-api -p client -p worker -p yoi-workspace-server`: passed including integration/doc targets. Unit suites: client 57, server-api 92, ticket 120, worker 975, Server 830 passed/1 existing ignored. The compaction integration target's 25 tests also passed.
- `cargo test -p server-api --features typescript`: 101 passed; all feature-catalog artifact tests passed.
- Root `cargo check`: passed, including normal TUI/Server closure.
- Canonical OpenAPI and all six OpenAPI-based generator `--check` runs: passed.
- `cargo fmt --all -- --check`, `git diff --check HEAD`: passed.

Initial signed-source tests failed on attribution/replay and passed after the fix. An attempted Deno format of auth catalog output failed its exact generated-artifact test; regenerating it in the documented raw form restored the artifact and all 101 feature tests passed.

## Naming audit limitation inherited from target

The required state-contract-name command was executed. It reports **30 unapproved lines and 9 stale exception hashes** in develop's frozen SSH migration/fixtures, migration-entrypoint fixture and timestamp-mismatch report. Every affected file and the exception inventory are byte-for-byte unchanged from `71ab4c06` (`git diff --exit-code 71ab4c06 -- <affected paths>` passed). The T-740 integration additions introduce no such finding. The inventory was not expanded, hashes were not silently refreshed, and no upstream historical contracts were renamed merely to make the checker green. This audit is **not reported as passing** and remains an Orchestrator/user follow-up outside the classification correction.

## Limits / handoff

T-739 is not integrated: its future case-state/close/migration composition is **not verified** by these tests. T-743 final Web UI, live production DB migration and full end-to-end UI/Runtime execution are not claimed. The existing ignored Server Drive Web-adapter roundtrip was not executed. The current-DB upgrade test covers a seeded schema-90 fixture, not live user data.

Only the existing source branch is to be published non-force. Source movement requires fresh trusted review on the same explicit MR `01a126a0-b4b1-7a13-ba61-5b2c06a3bca4` and current Ticket digest. Target publication, MR integration and Ticket completion/close are not performed here.
