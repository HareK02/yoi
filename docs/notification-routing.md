# Notification routing and evidence

Business notifications are derived from committed source facts. A callback is only a
low-latency wakeup: startup and bounded periodic recovery consume the same persisted
sources, decisions and requests. A notification neither grants authority nor changes
Ticket case/handling, MR approval, Job execution, or Worker lifecycle.

## Connection inventory

| Committed origin | Capture / decision owner | Recipient and negative outcomes | Delivery / recovery / evidence |
| --- | --- | --- | --- |
| Ticket Assign, Answer, item/content edits | Ticket transaction triggers → `notification_routing` | Original delegation; explicit request vs attention. No recipient, released/history, closed/done, unsupported recipient and self-notification are recorded separately. | Assignment reservations, pinned requests, authorized targets and ACKs; common recovery; `assignment_delivery_tests.rs` and `server_assignment_delivery_tests.rs`. |
| Ticket comments/post edits, tags, case, tracking, release | Same capture and routing ledger | They do not become new Orchestrator requests. Applicable implementation-Worker attention is distinct from new delegation. | Same evidence; `notification_routing_tests.rs` covers positive and negative routes. Unknown source kinds defer explicitly. |
| MR review requested/result/revoked/cancelled/merge and observed source changes | MR transaction/source-observation triggers → common routing | Ticket delegation and implementation recipient captured at the event. MR remains the freshness/approval authority. | Stable MR event identity; current MR must be reread before action. Ordinary review comments are not inferred handoffs. `implementation_result_tests.rs` and `server_implementation_result_tests.rs` cover capture, self-suppression, recovery and public diagnostics. |
| Explicit implementation result/handoff/blocker | Authenticated `TicketImplementationResult` transaction | Authenticated Workspace Worker and Ticket, without assignment membership requirements. The current Orchestrator is captured at report-save time, or no-recipient is recorded. No substitution of AssignmentAnswer or self-reported handling. | Result and source persist together; same common delivery/receipt path. No MR is required. |
| Backend Job accepted result | Result transaction → `notification_routing::jobs` | Fixed recipient, never replacement by the current Orchestrator. No target, checker no-findings and stale content suppress without ACK. Domain validation/rendering remains in the adapter. | Common request/ACK/failure ledger, fixed bytes/recipient, startup and periodic recovery; `job_notification_tests.rs` and `server_job_notification_tests.rs`. |
| Explicit WorkerNotify/WorkerSendInput | Authenticated caller | Explicit communication, not automatic business routing. | Existing Worker control boundary; not inferred from Ticket activity. |
| Internal SubWorker → parent / Web subscriptions | Parent-owned execution / UI synchronization | Separate ownership and authorization. | Not migrated into business-notification routing. |

The capture adapters and the central event-kind mapping must be updated together.
The tests intentionally exercise unknown kinds and missing source capture: neither
is represented as successful delivery or intentional suppression. Adding a producer
requires a positive persisted-receipt test and a negative/suppression test, not merely
an assertion that a send function ran. `notification_registration_tests.rs` checks
public MR event variants against the central mapping; comments are explicitly excluded.

MR source changes are captured when an existing Backend MR operation observes the
provider ref, not by an external Git-push listener. The initial source observation
is saved atomically with MR creation. `mr_open_retains_source_before_first_review_and_observed_result`
uses real Git publication and signed HTTP to cover the opening baseline, a later
source change before review, and result reporting against observation-only evidence.

Schema 96 migrates Job notifications; schema 97 installs MR/implementation-result
capture. Schema 98 removes assignment ownership and caller-supplied assignment IDs
from result reporting, preserving prior result bytes and notification evidence.
Legacy Job ACKs and suppressions remain distinct; comments never synthesize a result.
The model supplies only Ticket, kind, body and optional MR evidence. Host-side tools
use the durable Tool call identity as the transport replay key. Retrying the same
invocation cannot duplicate a report; a new invocation is a new report, so an unknown
outcome must not be retried blindly with a fresh call. Stored notifications retain
their captured recipient rather than being reassigned during delivery recovery.
Preparation failures remain retryable and visible after recovery; durable fair
scheduling prevents repeated failures from starving healthy or already-pinned work.

## Public diagnostic read

- HTTP: `GET /api/w/{workspace_id}/notification-diagnostics`
- Normal Tool: `NotificationDiagnostics`
- WIP: `notification_diagnostics` on the Ticket collection
- Shared DTOs: `crates/server-api/src/notification.rs`

Filters include Ticket, MR, Job, source event, assignment, delivery and exact source
identity. Pages contain at most 100 items. Continue with `next_cursor` and unchanged
filters. Cursors are validated against the Workspace and filters; they are not global
authority. A read uses a consistent database transaction and does not route, retry,
start/restore Workers or modify evidence.

`source_checks` reconciles requested origin references independently of pagination
and recipient filters. A present origin with no captured source is **unconnected**,
not suppressed. An MR-wide check reports whether *any* source was captured; it is not
proof of complete capture. Use the exact `mr-event:<mr-id>:<event-id>` reference to
check a particular event, including a later event after earlier successful capture.
Unsupported origin-reference forms report unknown presence, not absence.

The projection distinguishes awaiting routing, suppression, deferral, historical
recipients, pending/reserved/persisted requests, rejection, failures, uncertain
acceptance and durable Runtime acceptance. Decision/reason remain separate from
current delivery evidence and the last failure. Historical rows are not pending
work. `retry_candidate` is saved evidence, not authorization to send.

`acceptance_outcome_unknown` is conservative: a prepared request without a durable
ACK is sufficient, and does not prove a network attempt occurred. Later preflight
failure cannot disprove earlier acceptance. Multiple authorized assignment targets
without an ACK are reported as ambiguous rather than attributed to the original
Worker; an ACK identifies the accepted target. Old Job receipts are marked with
`legacy_evidence`, retaining the distinction between accepted and suppressed legacy
Completed records. No modern ACK is fabricated from suppression.

The response contains public references and bounded reason codes, not notification
body text, binding credentials, capabilities, or private session content. Runtime
acceptance is not model comprehension, successful handling, or permission to act;
`model_processing_confirmed` remains false without such evidence.

## Verification boundaries

Store fixtures use actual SQLite transactions, migrations, reopen, concurrent scans
and immutable records. HTTP tests use the authenticated production router. The
assignment and Job integration fixtures use the production Worker controller and
persistent session, substituting fixed model output and the deliberate response-loss
boundary. They verify request identity/bytes, one retained session notification,
restart recovery and agreement with public diagnostics. They do not establish real
model decision quality or unbounded exactly-once execution after Runtime receipt
retention expires.

Useful focused checks:

```sh
cargo test -p yoi-workspace-server --lib notification_diagnostics
cargo test -p yoi-workspace-server --lib job_notification
cargo test -p yoi-workspace-server --lib implementation_result
cargo test -p yoi-workspace-server --lib mr_result_recovery_and_handoff
cargo test -p yoi-workspace-server --lib notification_registration
cargo test -p yoi-workspace-server --lib assignment_real_worker_response_loss
cargo test -p worker --lib feature::builtin::ticket::notification_diagnostics
cargo test -p worker-runtime --lib worker_source::tests::
cargo test -p server-api
```

OpenAPI and the affected browser DTO artifacts are generated from the same public
contract. WIP and normal Tool paths share the typed operation and Profile permission
check; there is no separate WIP notification policy.
