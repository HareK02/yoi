# T-743 additional assignment Tools / WIP handoff

This is an additive follow-up to `T-743-prompt-surfaces.md`; that prior report/history is not rewritten. Additional scope follows T-743 thread sequence 6 and the parent request. Read current typed T-743 and inspected the backend child's actual Server DTOs/routes, not just the proposed names.

## Implemented public surface

| Normal Tool | Native Ticket item operation | Contract |
| --- | --- | --- |
| `TicketOrchestratorAssign` | `orchestrator_assign` | Explicit delegation/reissue to target principal; required observed nullable `expected_assignment_id`, stable `operation_id` |
| `TicketOrchestratorClear` | `orchestrator_clear` | Explicit release of observed assignment_id with stable operation_id; not Worker lifecycle |
| `TicketAssignmentsQuery` | `assignments_query` | Bounded retained Orchestrator rows, after_assignment_id / next_assignment_id |
| `TicketAssignmentDiagnostics` | `assignment_diagnostics` | Actual sanitized DTO with separate input/start cursors and unknown/history distinctions |
| `TicketAssignmentAnswer` | `assignment_answer` | Same current waiting/inprogress assignment, observed event CAS, stable operation_key, bounded body |

Reissue is the explicit Assign operation with a new caller-chosen operation identity against the observed old assignment. Exact replay retains the original key, full principal/CAS/input; no tool automatically queries a newer row, rewrites the expected identity or generates a new operation. All descriptions require querying current authority first. Target principal is operation data, never source/actor authority; the tools fix the target role to Orchestrator responsibility, leaving real role/principal/authentication checks in Backend. No worker-start/transfer operation was invented.

The new endpoints are the actual typed canonical definitions for `assignment-tracking/assignments`, `assignment-diagnostics` and `assignment-answer`. `WorkspaceClient` has no dedicated role-assignment method: the shared typed `WorkspaceServerOperation` forwarding surface was extended in the Worker crate, using actual `server_api` request DTOs and the existing host-bound execution authority. No client-crate extension is needed for these operations, no direct HTTP/storage fallback is used by the tools, and no second assignment/delivery/receipt ledger was introduced.

`QueryTicket` now accepts the actual typed current `assigned_principal`, `handling_status`, and current closed-case `closure_reason`. Closure reason remains separate from classification tag IDs. New read-only/review surfaces contain retained query and diagnostics, never Assign/clear/answer/self-report mutation. Mutation exposure is additive from the actual manifest authoring/thread/workflow flags; availability is not Backend authorization.

## Changed paths in this follow-up

- `crates/worker/src/worker.rs`: shared typed operation forwarding and scalar encoded query transport.
- `crates/worker/src/feature/builtin/ticket.rs`: feature inventories, query schema, native operation mappings and registration.
- `crates/worker/src/feature/builtin/ticket/assignment_public.rs`: new Tools, schemas, input validation and typed bounded/sanitized response handling.
- `crates/worker/src/feature/builtin/ticket/assignment_public/tests.rs`: new boundary/transport/WIP/access/profile fixtures.
- `crates/worker/src/prompt/system.rs`: effective actual-profile research return, explicit confirmation wait, same-row answer and generic close fixtures without repository/MR prerequisites; guarded integration fixture remains separate.
- `resources/prompts/common/tickets.md`: concrete tool names, query-first/CAS/exact replay/actor boundaries.
- `docs/development/ticket-assignment-tracking.md`
- `docs/development/ticket-assignment-delivery.md`
- `docs/development/work-items.md`
- `docs/design/ticket-wip-projection.md`
- this new report.

## Validation evidence

Passed focused commands:

- `cargo test -p worker --lib feature::builtin::ticket::`: **57 passed**, including 12 new public assignment/filter tests. These exercise required observed null/CAS, unknown model authority fields, same-input replay forwarding, read/diagnostic cursor encoding and bounds, answer body/identity validation, stale/permission/unknown outcome non-retry, diagnostic content/credential stripping, ordinary/WIP equivalence and route binding, read-only exclusions, and actual resolved builtin manifest access.
- `cargo test -p worker --lib prompt::system::tests::effective_`: **5 passed**, including the actual resolved-profile no-repo/no-MR research→waiting+answer→reasoned-close fixture and the independent guarded MR-review fixture.
- `cargo test -p worker --lib feature::builtin::orchestration::`: **7 passed**.
- `cargo test -p worker --lib feature::builtin::manage_worker::tests::worker_lifecycle_spawn_serializes_ticket_assignment_without_workdirs_or_flow`: **1 passed**. The prior concurrent macro blocker no longer prevented this test.
- `cargo test -p worker --test feature_invocation_test`: **13 passed**.
- Root `cargo check`: passed at the observed combined snapshot, including Runtime, TUI and Server compile closure.
- Direct delegated-file `rustfmt --check`, `git diff --check HEAD`, and naming check: passed (`558 matching lines; 0 unapproved; 0 stale exceptions`).

Root `cargo fmt --all -- --check` was **not clean** at the latest observation: non-delegated concurrent edits still needed formatting in `crates/worker-runtime/src/worker_source.rs` and `crates/workspace-server/src/authority.rs`. No outside-scope formatting write was performed. Parent owns the final combined formatting/validation pass.

## Limits and return

Transport/schema fixtures use a recording host-bound Workspace adapter; they prove concrete tool wiring, boundary input, permissions and non-retry behavior, not fresh signed Runtime→Server→DB→notification end-to-end execution or model judgment. Backend child's actual persistence/source-fence tests, generated contract synchronization, UI/browser/visual validation, all-Worker/full-stack tests, final combined acceptance and environment rollout remain parent-owned/separate. New API source definitions and one successful root compile are not whole-Ticket satisfaction evidence. No T-743 completion, review request, commits, pushes, merge, Worker stop or backend ledger implementation was performed.
