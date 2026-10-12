# T-743 → T-748: published domain-helper / caller handoff

## Source identity and composition

This is the retained pre-composition caller inventory published with T-743
`9b0d42fdbc2920736c3418bb894f26140e962bbc`. Its descriptions of direct callers
below identify the work to move, not the current execution placement. The later
normal merge, actual DB/Host placement, ten new regressions, full-suite evidence
and unresolved upstream review findings are in `T-743-T748-composition.md`.

This inventory belongs to the T-743 commit carrying this file on `work/T-743-ticket-public-surfaces` (T-742 base `56795fb92cb9def9bcd701bd51d92e99526f8eff`). Its exact published SHA and MR ID are recorded in the Ticket publication handoff. It is not a copy of the unpublished T-748 adapter and is not evidence that async DB execution has been integrated or reviewed.

Normal-merge the validated public T-748 source into this existing work branch. Keep the MR's source/target selectors. Recheck the concrete new call graph below against the merged implementation; no placeholder executor, second connection authority, transaction split, fresh integration branch or force push is needed.

## Three added synchronous Store signatures

Declared in `crates/workspace-server/src/store.rs` on `ControlPlaneStore`, implemented by `SqliteWorkspaceStore`:

```rust
fn page_ticket_assignments(
    &self,
    workspace_id: &str,
    ticket_id: &str,
    after_assignment_id: Option<&str>,
    limit: usize,
) -> Result<ticket::TicketAssignmentsPage>;

fn page_ticket_assignment_diagnostics(
    &self,
    workspace_id: &str,
    ticket_id: &str,
    assignment_id: &str,
    after_input_event_id: Option<&str>,
    after_start_operation_id: Option<&str>,
    limit: usize,
) -> Result<server_api::TicketAssignmentDiagnosticsPage>;

fn answer_ticket_assignment(
    &self,
    workspace_id: &str,
    ticket_id: &str,
    input: &ticket::TicketAssignmentAnswer,
    source: &TicketOrchestratorAssignmentSourceContext,
    actor: &str,
) -> Result<ticket::TicketAssignmentAnswerReceipt>;
```

Implementations forward whole calls respectively to `assignment_tracking::assignments_page`, `assignment_delivery::diagnostics_page` and `assignment_tracking::answer`.

The answer helper owns one IMMEDIATE transaction: `require_assignment_source` (including replay), current actionable delegation admission for a new operation, `ticket::sqlite_answer_ticket_assignment_in_transaction` (full input/source fingerprint, retained receipt replay, observed assignment/event, open-case gate, body event, audit attributes and immutable operation result), then commit. Keep this complete helper inside one accepted DB job. Do not validate/CAS on one side of an await and append/receipt on the other. The two read helpers use their existing connection/snapshot boundaries and bounded cursors; the diagnostics input and start cursors are independent.

## Added async HTTP callers and surrounding synchronous work

In `crates/workspace-server/src/server.rs`, `WorkspaceContractService`:

| Canonical caller | Browser forwarding caller | Whole Store operation |
| --- | --- | --- |
| `ticket_assignments_page` | `ticket_assignments_page_browser` | `page_ticket_assignments` |
| `ticket_assignment_diagnostics` | `ticket_assignment_diagnostics_browser` | `page_ticket_assignment_diagnostics` |
| `ticket_assignment_answer` | `ticket_assignment_answer_browser` | `answer_ticket_assignment` |

Canonical paths are `/workspaces/{workspace_id}/tickets/{ticket_id}/assignment-tracking/assignments`, `/assignment-diagnostics` and `/assignment-answer`; Browser equivalents use `/api/w/{workspace_id}/tickets/{ticket_id}/...`. Browser methods forward to canonical methods, so they must not independently repeat a mutation or transaction.

All three canonical methods currently call `resolve_workspace_ticket_reference` before their Store method. This uses Ticket authority/backend DB reads, so moving only the final Store method is not enough. The answer path also calls `authenticated_assignment_source`; its Runtime branch checks the registered current Workspace Orchestrator through `find_workspace_orchestrator`, while its Account branch derives actor/source exclusively from authenticated request context. Preserve source authentication and the in-transaction source fence; do not turn request target principal into source authority. Own/cloned workspace/Ticket/query/body/context/actor values must cross the common execution boundary without borrowing request frames.

After the answer returns committed, the current handler calls `find_workspace_orchestrator` and `dispatch_orchestrator_assignment_attention`. Account for the former's DB-backed projection lookup and the latter's Runtime/network dispatch in T-748's existing stage/lane design. Runtime/network waits must not occupy the sole DB worker. Keep commit-before-publication and durable scan/recovery semantics even if the awaiting HTTP request is cancelled or its response is lost. Do not automatically resubmit an accepted mutation with a new operation key.

## Query authority extension and all entry paths

`TicketQueryRequest` adds typed `assigned_principal`, `handling_status` and `closure_reason`. Existing `TicketAuthority::query_tickets(query: TicketQueryRequest) -> Result<TicketQueryResponse>` in `authority.rs` applies current role/principal/handling and current closed-case reason constraints in its SQL candidate query **before** limit/pagination. Query text separately searches retained closure judgments, including after correction/reopen.

The HTTP call graph remains `WorkspaceContractService::ticket_query` → `scoped_query_tickets` → `api.authority.query_tickets(query)`. Canonical and Browser query routing, including the pre-existing browser GET query adapter, converge on this authority. T-748 must cover the whole authority read/enrichment call, not just the added SQL fragment or only one UI route.

Normal Tool/WIP read/assign/release/answer calls forward typed `WorkspaceServerOperation` values from `crates/worker/src/worker.rs`, through `crates/worker-runtime/src/worker_source.rs`, to the same canonical HTTP contracts. Added variants are `TicketAssignmentSet`, `TicketAssignmentClear`, `TicketAssignmentsPage`, `TicketAssignmentDiagnostics`, and `TicketAssignmentAnswer`; `QueryTicket` uses the existing typed query transport with the added filters. The set/clear variants expose existing assignment Store authority rather than creating additional new Store helpers. Their existing T-748 execution-boundary changes must be retained when resolving merge conflicts.

No new Tool-specific DB adapter is needed. Frontend helpers in `ticket-operations.ts`, TicketHandling/Tags and list/Orchestrator loaders are consumers, never replacement authorities.

## Composition validation

Existing T-743 proof: full selected Rust crate suites, generated contract checks, Web/component/browser tests and real-Worker notification/processing synchronization. See `T-743-public-surface-validation.md` for commands and explicit limits.

After T-748 composition, add/run the narrow boundary checks for all three new HTTP operations and query paths under controlled DB lock/admission, queue/job bounds, overload response, stale/source refusal, rollback, accepted-job cancellation and exact replay. Retain cookie Origin and signed Runtime-binding denial tests, assignment source fencing, one-transaction receipt/event tests and canonical/Browser route equivalence. Root check and relevant full changed-crate tests must cover the combined tree. Then publish only the same source branch normally, verify provider SHA equals local HEAD, and request fresh independent review of that exact combined source.

Current publication is neither final independent approval nor target integration, Ticket/MR completion, live update or cleanup.
