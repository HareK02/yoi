# T-743 delegated Prompt / Profile / Flow / Worker handoff

## Boundaries

Base: `56795fb92cb9def9bcd701bd51d92e99526f8eff`, branch `work/T-743-ticket-public-surfaces`. Read typed T-743, O-19 and T-739–T-742, assignment tracking/delivery docs, generated OpenAPI and the Rust testing strategy before tests. This report covers only delegated resources/docs/Worker changes, not the parent's UI/Server/API work. No backend ledger, commits, pushes, merge, review request or Ticket conclusion was performed. Existing reports and history were not rewritten. Two historical design documents received only a supersession notice above their preserved original content.

## Changes

- Remove ready/intake-ready/Queue from normal Worker Ticket catalogs and native WIP mappings, including all-capability configurations. Remove obsolete queue-response observation invalidation; preserve inverse relation invalidation. Internal trait adapters are not new public operations.
- Align common/role/panel instructions: cases open/closed; stable classification IDs and actual tag/set CAS; assignment waiting/inprogress/done is self-report, independent of Worker/MR/delivery/acceptance/work release.
- Answers during waiting/ongoing handling continue the same assignment; explicit done reissue creates a new row even for the same principal and preserves old results. Reopen and Worker exchange alone are not reissue. Done/close do not require unassignment.
- Explain full-input operation identity/replay, observed assignment/event CAS, stale reread/reconciliation and unknown outcomes without automatic fresh-operation retry. Distinguish saved Assign from failed delivery. Supply observed `orchestrator_assignment_id` for Orchestrator starts, including generic spawn/existing-Worker assignment.
- Preserve explicit user approval conditions and guarded trusted review for MR integration, without a universal MR/human approval gate on generic closure. Flow terminal handoff is not case closure, handling done, work release or Worker stop.
- Add catalog exclusion and effective prompt/schema fixtures for research return, confirmation wait and reasoned closure. These are scripted instruction/schema checks, not model judgment or full-stack behavior tests.

## Delegated diff paths

Worker:
- `crates/worker/src/feature/builtin/ticket.rs`
- `crates/worker/src/feature/builtin/ticket/assignment_tracking.rs`
- `crates/worker/src/prompt/system.rs`

Resources:
- `resources/flows/coder-review.dcdl`
- `resources/profiles/intake.dcdl`
- `resources/profiles/orchestrator.dcdl`
- `resources/prompts/common/tickets.md`
- `resources/prompts/panel/orchestrator_idle_queue_notice.md`
- `resources/prompts/role/coder.md`
- `resources/prompts/role/intake.md`
- `resources/prompts/role/orchestrator.md`
- `resources/prompts/role/ticket-worker.md`

Current docs / historical-status notices:
- `docs/development/work-items.md`
- `docs/development/ticket-assignment-delivery.md`
- `docs/design/ticket-wip-projection.md`
- `docs/design/merge-request-wip-projection.md`
- `docs/design/ticket-evidence.md`
- `docs/design/flow-state-graph.md`
- `docs/design/workspace-kanban-orchestrator-runtime.md` (notice only)
- `docs/design/orchestrator-worktree-layout.md` (notice only)
- this new report

## Focused validation evidence

Passed:
- `cargo test -p worker --lib prompt::system::tests::effective_`: 5 tests (including new return/wait/close schema fixture).
- `cargo test -p worker --lib feature::builtin::ticket::`: 45 tests, including normal/WIP retirement, case CAS, tag schemas/transport, assignment tracking and relation observation tests.
- `cargo test -p worker --lib feature::builtin::orchestration::`: 7 tests, including observed delegation forwarding and resource/Profile boundaries.
- `cargo test -p manifest --lib builtin_profile::tests::`: 4 tests.
- `cargo test -p manifest --lib profile::tests::builtin_orchestrator_keeps_cleanup_tool_providers_enabled`: 1 test.
- `cargo test -p flow --lib builtin::tests::`: 2 tests; built-in recipe compiles and keeps current selector review contracts.
- Root `cargo check`, `cargo fmt --all -- --check`, `git diff --check HEAD`, and naming check (`558 matching lines; 0 unapproved; 0 stale exceptions`) passed at an earlier snapshot before subsequent concurrent Server/API edits. Worker files were formatted again after the final fixture addition; diff check also passed afterward.

One subsequent Worker spawn-contract invocation was blocked before test execution by a concurrent parent-owned API edit: `crates/server-api/src/lib.rs:3208` (`ticket_assignments_page`) used `#[context] context: ServerRequestContext`; the API macro requires `#[path]`, `#[query]`, `#[header]`, `#[body]`, `#[binary]` or `#[extension]` and rejected it. Command: `cargo test -p worker --lib feature::builtin::manage_worker::tests::worker_lifecycle_spawn_serializes_ticket_assignment_without_workdirs_or_flow`. No test result or final combined-tree compile success is claimed for that invocation. Parent must revalidate its current API/combined tree.

## Concrete API / runtime limits

At base `56795fb9`, generated `openapi/server-api.json` has assignment tracking/history and start fences but no assignment-delivery query/receipt HTTP endpoint. `list_pending_ticket_assignment_deliveries` is an internal Store method, not a browser or normal Tool/WIP route. Normal Worker/WIP Ticket inventories also have no Orchestrator Assign/reissue/clear operation; this change removes obsolete Queue rather than fabricating a replacement operation or ledger. The parent owns any public endpoint/Tool handoff needed to satisfy the whole Ticket. Instructions explicitly require reporting unavailable typed operations and forbid inferring delivery/acceptance from status or Worker state.

The granted live typed Ticket tools still returned legacy six-state projections for the requested reads; those reads established requirements/history, not evidence that the running environment has adopted the repository's new API. No environment update occurred.

Not run/claimed: whole Worker test suite, Web/browser/visual validation, generated-contract regeneration/check, live migration, model judgment evaluation, full-stack E2E, final combined-tree root compile after concurrent API changes, or T-748 DB-boundary validation. Parent's concurrent Ticket/Server/API/Web changes are excluded from this implementation's evidence.
