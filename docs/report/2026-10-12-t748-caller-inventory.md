# T-748: async caller ownership inventory and scoped repair evidence

## Current status and evidence boundary

MR `01a12d3f-e90c-7b02-9ed9-cf3bd2e822a1` seq13 requested changes for published
`ebce7ab7b1230654b8eb5340afca8de835f24f12`. The independent review confirmed the
revoke cancellation/publication fix, but found three further direct async callers.
Earlier full suites exercised successful outcomes without proving every composed
caller boundary. Repeatedly treating a helper's apparent purpose as its lane
classification missed Runtime lookup in Ticket projection and post-await reads.
A successful suite is not a substitute for the entry-to-publication inventory.

The seq13 and adjacent fixes were committed as `5873f250`; embedded protocol
and Feature shutdown fixes as `4d981194`. The remaining confirmed caller-placement
and Worker stop repairs are included in the follow-up below. The read-only audit
children were released after result-observation gaps; that release was not treated
as completion. No audit-to-implementation approval gate is imposed.
No target integration, Ticket conclusion, live update or attachment cleanup is
performed. T743 domain APIs are not copied or implemented here; their owner still
needs normal composition and exact combined-source review.

The tables and line numbers below preserve the **pre-repair audit snapshot**.
“Unfixed”, “pending”, and “remaining” in those historical sections describe that
snapshot, not the current implementation. The final section supersedes their
placement/stop status and explicitly distinguishes tested boundaries from limits.

Audited snapshot `server.rs` SHA-256:
`dae05a2649f5d4f884e8118ba4ce6baa902de6e61727c20e97c0afa3661ef3a6`.
Audited snapshot `worker_operations.rs` SHA-256:
`e3ad2327370440fdc40fb2c76f9604b984667e3472570d76145f4d0987d343fd`.
These file hashes identify the inspected implementation, not a provider review ref.

## Current confirmed ownership changes

| Entry / composed family | Authority and synchronous dependencies | Current owner / completion boundary | Evidence and limits |
|---|---|---|---|
| `ticket_implementation_cancel` → `scoped_cancel_ticket_implementation` | Ticket authority/assignment reads; Runtime cancel; current-session close; intact assignment cancellation transaction; detail | One accepted finite Host workflow owns cancellation through transaction and response. No Runtime wait on DB, no nested Host admission | Pre-authenticated generated POST route; real separate SQLite writer holds actual cancellation transaction; timer/control progress; cancelled waiter retains actual lease, then one operation/cleared assignment after drain. Saturated Host returns 503 without mutation. Existing live Coder cancellation policy passes |
| `merge_request_selector_repair` → `scoped_repair_merge_request_selector` | Browser auth/explicit confirmation; MR store; current assignment; provider ref; intact repair/audit; public projection | Auth remains an owned DB read; confirmed post-auth mixed stage is one finite Host job | Browser auth is drained before real SQL/shared-mutex contention; post-auth stage stays Pending without blocking Tokio and retains cancelled Host lease. Imported NULL-selector/no-assignment fixture rejects without repair/thread effects. Host overload preserves confirmation priority. Successful provider repair is not claimed by this narrow fixture |
| restore routes → `restore_runtime_worker_with_context` → `project_worker_restore_response_owned` | Existing authorized Host restore returns historical result; then whole stored Worker/links/Workdir/capability projection | Whole independent DB stage. A result with no Worker remains a pure response and needs no DB admission | Real SQL writer/shared-mutex hold; timer and independent Host control progress; bounded overload; cancellation retains actual DB lease; historical state preserved and no registry observation write. This tests post-restore policy, not a complete restore endpoint/E2E |
| Ticket detail/show/query and alias `get_ticket`; edit/event detail and assignment preparation | `TicketAuthority` projection includes open-MR `RuntimeTicketSourceCommitResolver` → synchronous provider HTTP | Mixed stages use finite Host, not sole DB. Subsequent pure assignment mutation stages keep their existing owner | Detail/query held loopback `/v1/repository-refs/observe`; unrelated DB/timer/control progress; cancel retains actual Host budget; overload stays 503. Provider-placement fixture is explicitly unsigned, not proof of signed request authority |
| Ticket tracking/case decision authority | Retained assignment/CAS/body/event checks plus `find_workspace_orchestrator` → live `RuntimeRegistry::worker` | Whole existing decision helper on finite Host; transaction and notification order unchanged | Held loopback live Orchestrator lookup for both tracking/case; independent SQL/timer/control; cancelled actual Host lease retained; failed live authority leaves Ticket unchanged. Existing non-assignee/browser bypass denial passes |
| Browser Ready | Existing mutation/notifications, followed by detail (including source ref observation) | Mutation and detail use one accepted Host job and explicit `execute_ticket_rest_operation_on_host`, never a second Host admission | Fake promptly denies notification GET, holds the subsequent source-ref POST; exactly one Ready event/readiness is durable before held detail; SQL/timer/control progress and cancelled Host ownership persist. Rejected fixture setup/misidentified notification GET are excluded |
| Worker control input/cancel/stop/restore source preflight | Synchronous live source Runtime lookup precedes `resolve_async` and operation admission | `authenticate_worker_mutation_source_owned` admits finite Host; already-owned synchronous callers retain the original helper | Actual control handler's held source GET proves DB/timer/control progress and cancelled finite Host budget; saturation returns availability, not source denial. The fake's lookup fails before an actual Worker cancellation; no product process is used |
| Worker observation capture | Source lookup, initial SQL grant, control lock, live grant re-fence and target Runtime lookup, awaited cold protocol connection | Source on Host; initial grant on DB; actual re-fence/live lookup on Host owns the acquired control guard; cold protocol signing retains existing DB stage | Source/permission/cold-signing contracts remain covered separately. Full capture success/held-protocol composition is not claimed by the new source lookup fixture |

## Regressions and causal checks

New `server_residual_execution_tests.rs` contains eight focused tests, including
three read/Ready and two decision-authority table cases. Exact old methods or the
old direct stored-projection placement were restored diagnostically in turn for
cancel, repair, restore, detail, tracking, and control. All six fail at their expected
first-poll/shared-mutex/independent-SQL boundary. Five-second fixture watchdog
release and its induced SQLite BUSY/panic are diagnostic bailout, not progress.
The product source is restored in `finally` after each case.

Artifacts (local, not tracked product code):
- `target/t748-residual-old-{cancel,repair,restore,detail,tracking,control}-red.log`
- `target/t748-residual-final-restored-green.log`: **26 DB-boundary tests pass**
- `target/t748-residual-decision-policy.log`: non-assignee/browser bypass policy passes
- `target/t748-residual-root-check.log`: root compile closure passes
- `target/t748-residual-provisional-full-suite.log`: original default-parallel Server
  **979 passed / 0 failed / 1 existing ignored**, binary11; **178.38 seconds**.
  Only the three deliberate executor unknown-outcome panic diagnostics appeared.
  No retained test/repro processes were observed after completion.
- `target/t748-residual-provisional-workdir-{default,http}.log`: default153 and
  http-client156 tests pass. This does not complete the pending transitive audit.
- fmt/diff and state-contract names **558 / 0 unapproved / 0 stale** pass.

All Cargo validation uses `target/t748-candidate-build`; no parallelism reduction,
production budget increase, skipped tests or mutation retry was introduced.
Initial test-only private enum/state/DTO mistakes failed compilation; missing
repository seeding and Ready notification GET confusion were diagnosed and fixed.
Those attempts are not semantic proof. The full-suite result above is provisional:
it does **not** conclude the caller inventory or approval.

## Remaining inventory (not complete)

`target/t748-async-entry-candidates.tsv` records 661 lexical async-entry candidates
from Server, Worker operations, auth, projection and subscription sources. Visible
lane markers/await counts are discovery hints, not an AST/type-checked call graph
or proof that all transitive operations are safe. Forwarders, actual handlers,
service adapters and callback/background callers must be matched by name and
traced through their real dependencies before marking a family inspected.

| Inventory work | Current status / next evidence |
|---|---|
| Ticket/MR/Objective/Memory/Subjektiv/Backend Job: canonical/alias/browser/Runtime entries, assignment/routing/decision/projection/publication and callback edges | First committed child result inspected. Major handler owner table returned, but follow-up final result was not observable. Parent released the child and is directly checking remaining adapters/forwarders/launch edges. Macro trait dispatch and representative Objective/Memory/Ticket forwarding plus legacy Memory/cleanup owners inspected; complete per-operation mapping remains open |
| Auth/Runtime/Worker/control/observation/resources/Workdir/repository/Drive/settings/config/catalog/uploads/sessions/subscriptions and lifecycle background edges | First committed child result inspected; parent verified the residual structures below against actual source. The follow-up result was not observable after two Idle notifications; parent released the read-only child and directly traced the remaining four edge groups below. This is not delegated completion evidence |
| Existing pure DB, mixed Host, mandatory Subject callback and accepted publication boundaries | Earlier documented evidence remains relevant; unchanged code does not exempt newly found composed callers from tracing |
| `WorkspaceWorker::stop` first Host result → waiter-owned session close → second Host stage | Parent verified the split in actual source. Structural ownership gap, not yet a cancellation reproducer; tracked below rather than silently marked safe |

## Additional residuals found before republication (unfixed)

The first two read-only audit results were inspected through their committed
sessions. The parent then read the concrete handler/helper source listed below.
These are **static execution-placement/continuation findings**, not new successful
endpoint, cancellation or overload tests. None has been repaired or published in
this snapshot. The provisional suite above therefore cannot establish safety for
these families. Lower recovery/transport edges are still being traced by the
parent after releasing the read-only children; this is not independent review.

`S` below means `crates/workspace-server/src/server.rs` at the frozen hash;
`O` means `crates/workspace-server/src/server/worker_operations.rs`.

| Entry / family | Entry → authorization → synchronous work → completion | Verified residual and required boundary |
|---|---|---|
| `repository_access_projection`, `repository_ssh_credential_{list,create,generate,get,delete,public_key,rotate}`, `repository_ssh_host_trust_{list,put,get,delete}` | ServerApi entries `S:7879–8107` directly await handlers `S:12934–13129`; owner authentication is awaited first, then active config projection / secret / trust / CAS / audit helpers execute synchronously | Auth ownership does not carry past await. Parent read all handlers and actual `repository_access.rs:391–436` connection reads. Whole existing synchronous operation needs an owner; pure DB and any external provider work must be classified by dependency, not by the SSH label |
| `repository_create` → `scoped_create_repository` | Owner authentication await → validation → `config_store.insert_repository` → intent/replay classification → response, `S:23157–23214` | Insert is outside an accepted job. Preserve the existing intact insert/intent/commit helper and successful replay vs conflicting intent semantics |
| `repository_ssh_connection_probe`, `repository_ssh_host_trust_confirm` | Owner and active binding awaits → catalog / `probe_repository_ssh_connection` → Runtime observation → trust read/CAS/write → response, `S:23247–23358` | Post-auth mixed work has no finite Host owner. Confirm must keep observed host-key selection and fingerprint comparison before the existing trust mutation |
| `worker_session`, `worker_session_history`, `scoped_worker_session_attachment` | Workspace checks → synchronous Runtime Session API → response/body, `S:8696–8769,12011–12069`; registry facade `hosts.rs:2129–2195` | No finite Host admission at these request boundaries. Binding/signing and remote transport dependencies must stay off Tokio and out of the sole DB lane; preserve Session diagnostics and response limits |
| `runtime_worker_attachment_upload` → `upload_runtime_worker_file_contract` | Grant claim → registry resolution → Runtime upload → grant completion or cancellation artifact deletion, `S:28081–28152` | Whole body-upload operation is still direct synchronous mixed work, distinct from the already-owned grant/cancel/delete routes. Admission must precede grant claim; accepted upload must own completion and cancellation cleanup |
| Drive Worker authorization / `drive_grant_create` | `server_drive_grants.rs:54–74,126–166`: source/header checks → `current_worker_identity` or live Runtime worker → later DB grant precheck/mutation | Pre-DB source/live Runtime lookup has no Host owner. `S:18244–18292` confirms `current_worker_identity` directly reaches synchronous `runtime.worker`. Retain no-actor-fallback policy; owner unavailability must not be misreported as authority denial |
| `current_worker_workspace_config_{get,attach,observe,read,commit}` | `S:11625–11699` directly invokes `workspace_config::identity`; `server_workspace_config.rs:65–74` reaches the same synchronous Runtime source helper before later owned DB stages | All five logical-config entries share the unowned identity preflight. Later accepted persistence/publication does not cover this earlier stage |
| `workspace_workdir_list` → `scoped_list_working_directories` | DB job `S:25943–25959` → summaries `32974–32999` → all Runtime workdir observations `33515–33524` → list/detail Runtime calls and SQL reconciliation `33421–33467` | Mixed Runtime/SQL helper is incorrectly on the sole DB worker. Preserve live reconciliation and diagnostics, but move the whole mixed stage to finite Host rather than pretending the list is stored-only |
| `workspace_config_tree_commit` → `scoped_commit_workspace_config_tree` | DB job `S:13444–13460` → prepare → repository-access projection → config commit → Runtime prompt publication, `13498–13517`; remote publication `hosts.rs:5090–5099` | SQL commit is followed by synchronous network publication inside sole DB job. Keep commit-before-best-effort-publication and accepted ownership; do not leave the continuation on the waiter. Logical config's separate owned Host dispatch is not this UI path |
| `repository_{list,detail,log}` and aliases | DB jobs `S:29108–29183` → live repository reader → summary/log provider; `repositories.rs:276–375` invokes Git subprocess for materialized local repositories | Provider work is not pure DB. Mixed catalog/provider work needs finite Host, without changing existing diagnostics for unmaterialized sources |
| `worker_control_stop`, `runtime_worker_stop`, `runtime_worker_stop_alias` → `WorkspaceWorker::stop` | `O:379–425`: grant/lock/re-fence → Host #1 Runtime stop returns result/control guard → waiter session lock/close → Host #2 linked Workdir observation/SQL | Structural ownership gap confirmed by parent: accepted Runtime stop has no owner for session close and subsequent Workdir synchronization if its waiter vanishes. `S:18609–18619` awaits real session close; the second Host admission also occurs after side effects. Whole accepted stop must own this continuation without nested Host acquisition. Cancellation reproduction remains pending |

### Parent takeover of the four remaining Runtime edge groups

The Runtime child's first result remains useful, but its follow-up final result
was not available through authorized observation. The parent did not infer
completion from Idle and did not loop Submit or inspect raw session storage.
See `docs/report/2026-10-12-worker-observation-followup-gap.md` for the bounded
observations and handling. The parent read the following actual source directly:

| Edge | Source and actual stages | Classification / unresolved contract |
|---|---|---|
| Embedded protocol connect and method pump | `S:31845–31902` directly calls synchronous `attach_worker_protocol_scoped` before spawning its async pump; the pump directly calls `send_connected_protocol_method_scoped`. `worker-runtime/src/runtime.rs:3486–3598` takes synchronous operation/state locks; Shutdown performs `begin_stop_operation` and `commit_stopped_worker` (`4359–4445`), with durable record/snapshot writes in fs-store builds (`5256–5269`) | **Additional confirmed placement residual**: Backend async connect/pump is not an accepted finite Host owner. The existing endpoint identity/settled-operation checks and commit/publication helpers must remain intact; no Runtime-store refactor is needed |
| Workdir creation reservation → Runtime result → finalization | `S:26316–26609`: DB preparation/reservation, Host runtime selection/observation/create, then waiter-admitted failure or final registry/receipt DB jobs. Same-id reentry observes the reserved Runtime Workdir before attempting create (`26473–26517`) | **Confirmed continuation split**, with durable same-operation recovery intent, not automatic mutation retry. Existing recovery does not make the original create job own finalization after its waiter disappears. Final classification/regression must cover accepted Runtime outcome, final receipt, cancellation and overload without creating a second provider operation |
| External grant fence → attachment/provider cleanup | `S:25595–25709`: DB authority/read or revoke/fence/audit, then waiter-owned cleanup/provider stop; terminal GET/revoke explicitly retries cleanup. Cleanup itself commits pending, awaits release, then admits completion/audit DB (`25719–25799`). Expiry task retains its async owner and retries (`24790–24854`) | **Confirmed continuation split plus explicit recovery**, not a claim that revocation failed or is unsafe after its DB fence. Grants remain denied by their committed fence. Need causal cancellation/lease evidence for cleanup and preserved retry/expiry policy before choosing the smallest ownership fix |
| Command provider acceptance → registration/audit | `S:19507–19631`: Host identity, DB attachment fence, async session/provider, then synchronous command-handle registration. External `operate` admits audit-requested DB, sends provider request with explicit cancel guard, receives result, then admits audit-completed DB (`888–1024`); External command-handle registration follows another await (`1372–1428`). Local command start retains Runtime resources in its actual task and owns terminal-close admission (`workdir/src/local.rs:1831–1897`) | **Partially protected / continuation question**. SQL jobs are owned and local actual command resources are retained; explicit external cancel is not SQL retry. Provider acknowledgement-to-registration can cross a cancellable audit await. Do not assert an orphan process or whole command success without a focused protocol/registration reproducer; do not rewrite external cancellation semantics speculatively |
| Catalog and single-workspace shutdown | `S:12222–12279`: catalog waits for HTTP serve, drains Worker projection and owns Feature shutdown DB (`2651–2664`); single-workspace `serve` aborts its hook but calls synchronous Feature shutdown directly. `feature-storage/src/lib.rs:220–241,521–523` fences and waits on connection close. Executor `drain` (`db_executor.rs:290–315`) covers only jobs accepted before its first poll, not later submissions | **Additional confirmed direct shutdown placement residual** in single-workspace serve. Separately, catalog does not demonstrate hook/expiry/Host/cleanup producer quiescence before storage shutdown. Whole producer/drain ordering remains a lifecycle question; no live shutdown was invoked to test it |

Domain generated adapters, remaining Tool/WIP forwarding, Backend Job / legacy
Memory launch lower edges remain under follow-up inspection. Existing domain SQL
helpers are not being reimplemented or globally refactored; ownership tracing is
separate from re-auditing every SQL semantic branch. None of the additional rows
above has new executable regression evidence yet.


T743's additional assignment page/diagnostics/answer APIs are absent from this
source. Ticket seq8–9 identifies their separate owner's normal composition with
the published T748 facade. This inventory is **not** an attestation of that future
combined source; no duplicate API or combined approval is introduced here.

## Republication conditions

Review-ready requires a consolidated caller-name list with each entry's auth,
SQL/provider/Runtime stage, post-await projection, post-commit publication and
owner identified; confirmed residuals repaired with focused causal regressions;
remaining uncertainty explicit; and final root/changed-crate/affected-feature,
format/diff/name checks on that final source. Only then normal same-selector push,
provider exact-HEAD verification and fresh trusted independent review of this MR.
No new Reviewer Runtime or separate Coder is created. Read-only audit children do
not grant review verdict or integration authority.

## Scoped caller repair and validation history

The same Ticket branch is retained for advancing the existing MR. This section
records validation history, not approval or integration. The final-validation
section below supersedes the intervening failed runs without deleting their evidence.
The existing domain helpers,
authorization fences, receipt/replay rules, and transaction/publication order
remain intact. No automatic mutation retry, production limit increase, nested
Host admission, or test parallelism reduction was introduced.

| Repaired family | Current owner / executable evidence |
|---|---|
| Repository access/SSH credential and host-trust CRUD, Repository registration | Whole post-owner-auth synchronous operation on DB. Registration regression observes the **config store's actual connection** waiting behind a separate SQLite writer, then cancels the waiter; after drain exactly one Repository exists and the same registration intent returns HTTP200/replayed without another row |
| SSH probe/host-trust confirm | Whole post-owner/binding-auth mixed probe/observed-key selection/fingerprint comparison/trust mutation on Host. Existing narrow SSH/trust policy tests pass in the full Server suite; no successful real SSH connection or host-key exchange is claimed |
| Repository list/detail/log and aliases | Live Git/catalog helpers on Host, not DB. With DB admission saturated, empty catalog and absent-key detail/log retain their ordinary success/404 responses. This regression proves lane selection, not held Git subprocess behavior |
| Worker Session/history/attachment | Typed Session service and attachment handler admit Host before synchronous Runtime observation. Loopback held Runtime lookup proves independent SQL/timer/control progress, cancelled actual lease retention, bounded overload and release after actual drain |
| Body upload | Host admission precedes the upload grant claim and owns resolution/upload/completion/cancellation cleanup. The same loopback table holds the Runtime lookup after claim; cancelled waiter leaves actual owner, and provider refusal completes the claim back to Pending. Successful artifact upload/delete and established remote sessions are not exercised by this refusal fixture |
| Drive Worker authorization / grant create; all five logical-config identity entries | Live source/target preflight uses Host; later DB authorization and mutations retain their previous transaction owner. Three held-lookup table cases prove independent SQL/timer/control, cancelled lease retention, and typed HTTP503 availability (not false authority denial) when Host admission is full. Existing no-actor-fallback and source-fence policies remain in the suite |
| Workdir list | Whole live Runtime reconciliation/SQL helper on Host. Held Runtime list lookup in the Session/Workdir/upload table proves sole DB remains free and cancellation retains the actual Host budget |
| UI config commit | Whole existing evaluate/project/commit/best-effort prompt publication helper on Host. Loopback held prompt POST confirms the changed tree is already durable while independent DB/timer/control proceeds. Cancelling the waiter retains publication ownership; refused publication neither repeats nor rolls back the commit |
| Worker stop canonical/alias/control entries | One accepted Host workflow owns Runtime stop, session fence/close, stored read and linked Workdir synchronization. An in-process Runtime emits its stop receipt while a real session fence is held; after waiter abort and fence release the registered session is removed. Restoring the exact old stop method makes this test fail at the missing cleanup assertion; restoring the fix passes. No real Worker/product process is spawned or stopped. Linked Workdir provider success is not claimed by this session-only fixture |
| Embedded protocol connect/method; single-workspace Feature shutdown | `4d981194` owns these operations on the existing finite Host/DB lanes. Focused protocol and actual Feature connection shutdown regressions remain green in the final suite |

Final logs in the Workdir (build artifacts, not committed product content):
- `target/t748-stop-old-placement-red.log`: old stop fails **missing cleanup**, not timeout.
- `target/t748-stop-restored-green.log`: restored accepted-stop ownership passes.
- `target/t748-caller-host-green.log`: Session/history/attachment/Workdir/upload table passes.
- `target/t748-repository-create-green.log`: actual SQLite commit/cancel/replay passes.
- `target/t748-drive-config-preflight-green.log`: all three live authority cases pass.
- `target/t748-ui-prompt-owner-green.log`: commit-before-publication passes.
- `target/t748-repository-provider-green.log`: all three Repository read lanes pass.
- `target/t748-caller-final-root-check.log`: workspace-root `cargo check` passes.
- `target/t748-caller-final-server-suite.log`: original default-parallel Server
  **988 passed, 0 failed, 1 existing ignored**, plus **11 binary tests**, 185.38s.
- `target/t748-caller-final-workdir-default.log`: **153** tests pass across targets.
- `target/t748-caller-final-workdir-http.log`: **156** tests pass across targets.
- fmt/diff checks pass; `target/t748-caller-final-names.log`: **558 / 0 unapproved / 0 stale**.

Test setup errors are excluded from semantic evidence: the first stop fixture
omitted the Runtime's live Worker summary; the Repository busy observer initially
watched the general store rather than the config store's distinct connection.
Both were corrected before the successful runs. Missing package/Python command
and compile-time DTO/Duration/Drive-error mistakes also prove no product behavior.

### Explicit limits, not an expanding audit gate

The four lower-edge rows above separate confirmed direct placement repairs from
Workdir create / external grant cleanup / provider acknowledgement continuation
questions and catalog producer quiescence. The latter are not redesigned here,
per the user's instruction not to expand speculative Workdir/grant or unrelated
Feature work indefinitely. Their existing durable recovery, explicit cleanup
retry and provider cancellation policy are preserved, not reclassified as
end-to-end cancellation proof. This report is not a mechanically complete
transitive call graph or an assertion that every future/composed caller is safe.
T743's separate new API composition still requires its owner's exact combined
source validation/review. None of these limits constitutes Ticket conclusion,
MR approval, target integration or permission to update the running environment.

### Post-suite focused validation blocker

After the successful full suite above, the current candidate was checked with
`cargo test -p yoi-workspace-server --lib db_execution_tests` using the same
candidate target directory and ordinary parallel execution. Result: **31 passed /
1 failed**, in 8.06s. Failure:
`cancelled_runtime_key_revoke_commits_audit_and_removes_broker_registration`
reported the shared `observed` helper's two-second `Elapsed(())` timeout, without
identifying which of its several lifecycle waits timed out. Source inspection
confirms projection shutdown is already present in this fixture; it is not
legitimate to attribute the failure to omitted projection shutdown, treat it as
load-only, increase limits, reduce parallelism, or rerun until green.

Evidence is preserved in `target/t748-caller-final-boundaries.log`. The prior
full-suite success is real but insufficient to declare final validation complete.
The new caller and stop cases all passed in this focused run. Next diagnosis must
identify the exact wait and inspect actual HTTP/job/broker ownership events before
choosing a fixture or product repair. No normal push, fresh review, approval,
integration or Ticket conclusion was performed for this candidate.

## Continued diagnosis and final validation

Returning the failed test as a stop condition was a workflow mistake, not an
external outage, lost authority, or additional audit approval gate. Work continued
on the same branch. The failed run above is retained as evidence.

The `observed` helper now reports its caller location. A diagnostic 32-case run
passed; that result alone was **not** treated as a diagnosis or used to erase the
failure. Source tracing identified an uncontrolled cold broker signing producer
in the revoke fixture. The test now receives the actual signed WebSocket
handshake and drains its completed DB work **before** holding the revoke writer.
The handshake remains pending, so broker shutdown is still tested. An early HTTP
response is selected against actual SQLite entry, reporting status/body rather
than waiting for an event that can no longer occur. The original failed run did
not identify its exact wait; attributing it conclusively to signing would exceed
the captured evidence.

Subsequent full runs exposed the Ready table's distinct failure **before** source
POST arrival (caller location, not timer/control failure). Artifacts:
`target/t748-post-fixture-server-suite.log` and
`target/t748-final-framing-server-suite.log`. The HTTP fake previously assumed
one TCP read contained the request line and answered without consuming a body.
It now parses the complete header/body; a deterministic two-byte-fragment reader
proves complete frame consumption and truncated-body rejection. This is a real
fixture flaw but does not by itself establish the historical timeout cause.

Ready setup includes mutation and best-effort notification before the source
POST. The held-provider arrival now uses the existing five-second fixture
watchdog as its setup bound, selected against handler completion. The subsequent
**timer and independent SQL/control observations retain their two-second budget**;
actual held-request cancellation/lease/overload/cleanup assertions are unchanged.
Production admission limits, normal test parallelism, and provider timeout are
unchanged. This setup-bound change is explicit, not a hidden limit increase or
claim that Ready completes within two seconds under a whole-suite workload.
Restoring the exact published old Ready method still fails this revised test
(`target/t748-ready-old-boundary-red.log`, Ready returns only after synchronous
provider wait); restored code passes, so the offload distinction is not weakened.

Final code validation after these fixture changes:
- `target/t748-boundary-setup-fixed-green.log`: **33 DB-boundary cases pass**.
- `target/t748-final-setup-root-check.log`: root compile closure passes.
- `target/t748-final-setup-server-suite.log`: original default-parallel Server
  **989 passed / 0 failed / 1 existing ignored**, **11 binary tests**, 189.66s.
- `target/t748-final-setup-workdir-default.log`: **153 pass** across targets.
- `target/t748-final-setup-workdir-http.log`: **156 pass** across targets.
- `cargo fmt --all -- --check`, `git diff --check HEAD`, and state-contract names
  **558 / 0 unapproved / 0 stale** pass.

No failed semantic run was merely repeated until green: the intervening source
changes and the limits of each diagnosis are recorded above. These results support
publication for fresh exact-source independent review, not reviewer approval,
MR integration, Ticket conclusion, or updates to the live dogfooding environment.
