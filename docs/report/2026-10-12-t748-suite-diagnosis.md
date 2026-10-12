# T-748: default-parallel suite failure diagnosis

## Sources and preserved observations

- Original local candidate: `61533fadb6b39465af60cb55aed2b4d0650319cd`.
- Comparison source: `56795fb92cb9def9bcd701bd51d92e99526f8eff` (develop/T742).
- Same implementation branch: `work/T-748-bounded-db-execution`.
- The original `target/t748-server-tests.log` remains untouched. Its 97 FAILED
  lines and 600-second timeout were symptoms, not proof of one cause.
- Initial diagnosis used `cargo test -p yoi-workspace-server -- --nocapture`,
  default libtest parallelism, without changing queue limits, features, or startup
  settings. `target/t748-diagnostic-environment.txt` records the environment;
  this host reports 24 available CPUs. Diagnostic captures were bounded to
  90/150 seconds to obtain errors before the original timeout, not counted as
  successful full-suite runs.

Logs under `target/` are local diagnostic artifacts, not tracked review inputs.
This report records their meaning and the permanent regressions that reproduce it.

## 1. Different fixtures consumed one global Host admission budget

The first non-intentional errors in `target/t748-server-nocapture.log` were
`DbCapacityExceeded` in manual Worker setup (`server.rs` at the then-current
line 50614), followed by assignment fixture construction (52990/53045) and
HTTP 503 responses. The deliberate before/after-commit executor panic tests
are different: those passed and verified unknown-outcome diagnostics.

Temporary submit/start/cleanup instrumentation recorded submitting test names,
closure types, actual job IDs and the execution-owner identity. It showed a
single Host lane with `max=18`, with accepted jobs from unrelated fixtures.
A rejection snapshot contained `{32,45,46,...,61}`. The complete diagnostic
capture later contained cleanup for every submitted Host job on that owner;
it did not show permanently abandoned accepted Host jobs. Instrumentation was
removed before the source was committed/published.

`target/t748-live-process-18.txt` sampled the test process and threads at the
actual rejection. That sample had 126 threads; the Host workers were runnable,
and the process was using approximately 17 CPU cores. There was no captured
thread-creation EAGAIN/unavailable error. This evidence does not establish an
OS thread/CPU hard limit as the cause. No test processes remained after the
bounded diagnostic run or the completed suite.

A representative failure passed alone (0.56 seconds); the eight logging cases
passed with four test threads (2.46 seconds). The isolated comparison source's
full default-parallel suite passed: 931 library tests, one pre-existing ignored
migration fixture, 11 binary tests; library duration 168.74 seconds. This did
not prove that all new failures were capacity-related, but did distinguish the
new shared-admission rejection from an observed baseline failure.

### Ownership fix (`f3d4aba5`)

Production budgets remain shared and finite: DB 1 worker/32 queued, Host 2/16,
cleanup 2/16. Ordinary fixtures now explicitly own a finite DB executor and lazy
Host/cleanup owners at those same limits. API clones, child Workspace APIs, and
weak retention adapters use their injected owner. No global fixture lane cache,
parallelism reduction, unlimited queue, or capacity-wait retry was added.

The permanent saturated-Host regression fills both running slots and all 16
queued slots, cancels waiters, advances paused Tokio time, and still observes
rejection until actual completion. Another fixture and the separate cleanup lane
can progress; draining restores the original budget and completes exactly 18
accepted jobs. Existing real-SQLite cancellation/lease/drain tests remain active.

## 2. SQL offload invalidated fixture assumptions about first poll and publication

After Host isolation the default-parallel suite finished in 171.57 seconds:
954 passed, three failed, one existing ignored. Actual errors were:

- removal subscriber saw a legitimate queued Runtime stop Upsert before Removed;
- external session-open expiry exceeded its one-second bound while its DB lane
  was still shared across other fixtures;
- grant replacement returned success because one initial poll had only submitted
  the offloaded authority read, not reached the held original-grant lock.

The ordinary DB fixtures were isolated too. The one-second expiry assertion was
not increased. Grant testing now drains its actual authority read then polls the
saved response to reach the held lock before replacing the grant. Removal testing
observes the committed stopped projection before taking the removal baseline.
The Removed payload assertion is unchanged. These are observable synchronization,
not sleeps or retries. Focused expiry, grant, and removal tests all passed.

A captured cleanup job also attempted to poll a Tokio timer after its fixture
runtime had shut down. The terminal checker fixture now retains that original
runtime through accepted Host and cleanup jobs, then stops projection producers
and drains actual DB work before ending. Terminal SQL state is not treated as
actual cleanup completion. The subsequent full run contained only the three
intentional executor panic diagnostics, not the shutdown-context panic.

The synchronization/teardown changes are in `df9d056d`.

## 3. Restore intent's DEFERRED read-to-write upgrade returned SQLite BUSY

The next default-parallel run finished in 171.48 seconds with 956 passed, two
failed, one existing ignored. Both failures were SQLite `DatabaseBusy`, extended
code 5, not capacity rejection:

- production-profile zero-input Orchestrator restore;
- pending manual Worker claim restore reconciliation (500 instead of 409).

Both failed alone too. `target/t748-busy-origin.log` captured the error and a
backtrace through `pin_internal_worker_restore_intent` -> the transaction helper
-> internal restore -> manual claim. The journal read authority first in a
DEFERRED transaction while the offloaded projection's other SQLite connection
could write. Such an upgrade can return BUSY without invoking SQLite's busy
handler; treating this as a retryable whole mutation would be incorrect.

A new real-SQLite regression holds an IMMEDIATE writer on another connection,
registers an owned per-connection busy observer, and attempts one journal pin.
The old helper fails deterministically with BUSY/5 before waiting for ownership
(`target/t748-deferred-upgrade-red.log`, 0.23 seconds). The fixed helper waits
before its first authority read, while a current-thread timer and control task
progress. After releasing the writer it persists exactly one requested tuple;
there is no application-level mutation retry.

`f597f1b5` makes this existing mutation helper explicitly
`with_immediate_transaction`. Restore journal and deletion authority reads,
mutation and commit remain in one transaction. Focused DB/HTTP tests (8), the
original restore and claim cases, and deletion tests (13) passed.

## 4. Why a pre-dispatch failure could become the 600-second suite timeout

The original long-running logging case joined an HTTP request with a fake provider
waiting for command dispatch. On pre-dispatch rejection the HTTP future can finish
with 503 while the receiver remains Pending: its channel is still owned by the
fixture, but no command will arrive. An unbounded `join!` waits forever in that
state and hides the actual error behind the outer suite timeout.

The permanent `saturated_host_before_command_start_returns_availability_without_a_provider_operation`
regression uses the real signed HTTP route with a controlled full Host budget. It
observes the final 503/`db_capacity_exceeded` response and the still-live Pending
provider receiver, with no process or command dispatch. The original logging
fixture now asserts an early HTTP response before waiting for both branches, so
an unexpected rejection reports its body immediately rather than hanging. This
reproduces the causal timeout mechanism; the old log itself has no stack trace
from the hung provider future.

The nine logging cases pass together at default parallelism (`49bfbfb2`).

## Artifact/source isolation during comparison

Initially the archived baseline used the existing Cargo target directory. That
replaced a same-package artifact/fingerprint; one later post-fix representative
command ran the baseline's 932-test binary (and `db_executor --list` had no tests).
That result was rejected as candidate evidence. All later candidate checks/tests
use `--target-dir target/t748-candidate-build`, separate from the baseline cache.
This changes artifact location, not test parallelism, features or admission policy.
The fresh candidate build listed the expected executor tests and total test count.

## Validation and remaining composition boundary

Pre-review code source `49bfbfb2` passed the full default-parallel Server suite,
including the command-overload case: 960 passed, zero failures, one existing
ignored migration fixture; 11 binary tests, library duration 175.29 seconds.
The only captured panics were the three deliberate unknown-outcome tests.
Root `cargo check` compile closure passed (including TUI/Server), as did Workdir
default (153 tests) and `http-client` (156 tests) full suites, format/diff checks
and state-contract names (558 matches, zero unapproved, zero stale exceptions).
These commands used the isolated candidate artifact directory; libtest default
parallelism and the production budgets were unchanged. The MR records the exact
published source, which also includes this report and the boundary document.

## Post-publication independent review and causal fixes

MR `01a12d3f-e90c-7b02-9ed9-cf3bd2e822a1` seq3 independently reviewed published
source `66bb9d0b` and requested changes despite the successful original suite.
Its two major findings exposed boundaries the earlier tests did not prove:

- `T748-DB-NETWORK-LANE`: mixed Subjektiv helpers could wait for synchronous
  Runtime HTTP on the sole DB worker. Fix `d3fcfb29` places session read/search,
  subject projection, memory staging, and legacy consolidation-scope helpers on
  the existing finite Host owner; pure DB-resident operations keep DB placement.
  Whole transaction helpers remain intact, and no Runtime I/O is introduced
  inside a SQL transaction.
- `T748-ASYNC-PROTOCOL-SIGNING`: cold protocol source lookup called live binding
  checks and signing directly on Tokio. Fix `c0223a0d` awaits the owned DB stage.
  The source API now propagates failure explicitly: an invalid live binding or
  signing/admission failure cannot be hidden as `None` or a legacy bearer fallback.

Permanent real loopback HTTP regressions hold upstream Runtime replies during
session read/search, subject and legacy-scope requests, while independently
observing SQL, timer and control progress. Cancelling waiters retains the actual
Host lifecycle lease until completion. Cold protocol source tests control another
SQLite writer and shared connection mutex wait, and verify bounded overload,
fresh signing and revocation without fallback. Existing queue/cancellation and
deletion evidence now observes the actual mutation owner; the mixed fixtures
explicitly inject a small finite Host budget. Production budgets are unchanged.

Focused boundary tests passed (10). Restoring the old placement produced a
failing regression, and fixed placement passed; an earlier syntax-rejected
instrumentation attempt is excluded from evidence. Diagnostic artifacts under
`target/` are not production source or replacement domain implementations.

Exact code source `c0223a0d` then passed root `cargo check` and the original
**default-parallel** Server suite: **962 passed, zero failed, one pre-existing
ignored**, 11 binary tests; library duration **173.36 seconds**. Only the three
intentional executor panic diagnostics appeared, with no residual test processes.
Workdir default (153) and `http-client` (156) full suites, format/diff checks and
state-contract naming (558 matching lines, zero unapproved, zero stale) passed.
All used the isolated `target/t748-candidate-build` artifact directory. Logs:
`target/t748-review-fix-{root-check,full-suite,workdir-default,workdir-http,format,names}.log`.
The documentation-only publication `b10b7315` did not change that tested code.
MR seq5 independently confirmed the two old fixes and repeated the checks above,
but requested changes for a new mandatory-callback cycle; see the next section.

## Mandatory callback must be independent of its waiting Host parent

MR seq5 finding `T748-HOST-CALLBACK-CYCLE` traced Host create/restore -> Runtime
bootstrap -> mandatory Session attribution -> the same finite Host lane. Two
waiting parents could consume both workers while successfully admitted callbacks
could not run. Cancelling a waiter correctly retains the actual parent, so it is
not a solution. The Reviewer's compiled executor artifact proved this placement
cycle, not a complete create endpoint/E2E reproduction.

Fix `a2487b57` separates store-only scope discovery into typed Subject, current
Job consolidation and legacy consolidation authority. Session attribution accepts
only Subject authority and executes its whole existing SQL operation on the
independent DB lane. It rejects consolidation roles without calling Runtime.
Mixed read/search, subject projections and allowed legacy consolidation operations
keep their Runtime checks on Host; the previous DB/network fix is not undone.

The same dependency audit identified Subject resident context reads used by
Runtime bootstrap/restore. All Subject-only memory operations use store-only
Subject authority and DB ownership too (resident summary/context, proposal
validation, receipt lookup, explicit staging). Other memory operations retain
Host ownership because legacy authority can require synchronous Runtime lookup.
No transaction is split across an await and no production budget is increased.

The strengthened signed route regression holds both actual production-sized Host
workers (2/16) in `run_host_workflow` parents and retains their create admission
leases until callbacks finish. Resident and attribution routes authenticate real
Runtime source proofs, check current singleton ownership, respond successfully,
and persist both distinct Subject session attributions **before** either parent
releases capacity. It also preserves Session callback reentry during deletion
drain. Runtime's wait is modeled at this policy boundary; no product process or
complete create endpoint is spawned. Separate denial coverage proves legacy
consolidation callbacks return 403 without Runtime lookup, leave no attribution,
and do not bypass authentication (401).

Only Session attribution is an allowed deletion-drain callback; that endpoint
allowlist was not expanded. An initial fixture sent resident reads during deletion
and correctly received 409. The fixture now tests those reads before deletion,
then Session attribution while deletion waits. This is alignment with the existing
fence, not a relaxed product gate or success-picking retry. A diagnostic Python
invocation was unavailable before making any source change; only the subsequent
Deno lane-restoration runs are causal red evidence.

Restoring just Session's old Host placement fails at its expected callback
watchdog; restoring just resident memory's old Host placement fails at its own
watchdog. Fixed placement succeeds without parent bailout. Evidence:
`target/t748-callback-old-{session,resident}-placement-red.log`,
`target/t748-callback-restored-green.log`. Final focused signed callback tests (5),
DB boundaries (10), and Subjektiv regressions (25) passed.

Exact code `a2487b575df1f348251214c53da61595860fcb40` passed root `cargo check` and
original default-parallel Server **963 passed / 0 failed / 1 existing ignored**,
11 binary tests; library duration **173.01 seconds**. Only the three intentional
executor panics appeared, with no residual test processes. Workdir default153 /
http-client156, fmt/diff and naming558/0/0 passed. The isolated candidate cache
remains in use. Logs: `target/t748-callback-{root-check,full-suite,workdir-default,workdir-http}.log`.
The docs-only publication `27b6c688` changed no tested code. MR seq7 independently
confirmed the callback fix and earlier fixes, but identified another direct async
caller; its independent verdict was `request_changes`, not approval.

## Worker discovery and adjacent metadata caller audit

MR seq7 major `T748-ASYNC-WORKER-DISCOVERY` traced async discovery directly into
synchronous `workers_response`. That helper acquires the shared SQL connection
for the Worker projection snapshot, Workdir registry/links and authority projection.
The Reviewer's independently compiled public snapshot helper reproduced a
300.246ms shared-mutex stall while its Tokio timer could not progress. This was
helper-boundary evidence, not an authenticated endpoint/E2E run.

Fix `dbcea6c3` awaits the **whole existing** `workers_response` on the independent
bounded DB owner. Source-proof requirement, Workspace/query/cursor validation,
visibility filtering, pagination and control-grant behavior are unchanged. The
other production callers were traced: Worker list/resource detail already use DB;
mixed Runtime cleanup plans use Host. No network work is added to the DB owner.

The adjacent metadata audit found async `workspace_runtime_resources_response`
also directly calling synchronous Runtime summaries (including remote signed HTTP
and its awaited DB authorization). This call now uses finite Host ownership,
then the existing independent async binding read. Runtime list/detail share this
boundary. Whole SQL helpers, admission budgets and authentication fallback rules
are not changed.

The permanent post-auth discovery handler regression uses a separate SQLite
IMMEDIATE writer and busy observer to hold the actual shared connection mutex.
While the accepted discovery read is queued, current-thread timer and independent
Host control progress. A second read receives 503/`db_capacity_exceeded`; a source
without proof receives 403 before admission. Cancellation retains the actual
queued read's lease/budget until result cleanup; ordinary completion preserves the
empty filtered catalog response. Signed route proof verification remains covered
by the existing dedicated-permission test, separately from this post-auth policy
boundary so middleware's own DB wait cannot hide the handler bug.

Restoring the exact old discovery call fails its first-poll assertion after the
fixture watchdog releases the synchronous mutex wait. Restoring only the old
Runtime-summary placement fails because Tokio cannot observe the real loopback
request until the upstream watchdog releases it. Fixed summary placement lets
SQL/timer/control progress during the held signed upstream request and keeps the
actual Host lease after cancellation. Explicit upstream-release observation
prevents watchdog failure cleanup from masquerading as progress. Logs:
`target/t748-discovery-old-placement-red.log`,
`target/t748-runtime-summary-old-placement-red.log`,
`target/t748-discovery-summary-fixed-boundaries.log` (12 passed),
`target/t748-discovery-summary-policy.log` (4), and callbacks (5).

Exact code `dbcea6c34ba55453ce588d68272df238cde19f47` passed root `cargo check` and
original default-parallel Server **965 passed / 0 failed / 1 existing ignored**,
11 binary tests; library duration **175.00 seconds**. Only three deliberate
executor panics appeared, with no residual test processes. Workdir default153 /
http-client156 and fmt/diff/naming558/0/0 passed. Logs use the same isolated
candidate cache: `target/t748-discovery-summary-{root-check,full-suite,workdir-default,workdir-http}.log`.
Docs-only publication `b63bdb44` changed no tested code. MR seq9 independently
confirmed these fixes, then requested changes for Runtime management's residual
synchronous authority calls; it was not an approval of the complete source.

## Runtime management: authority, network, and accepted publication ownership

Seq9 major `T748-ASYNC-RUNTIME-MANAGEMENT` identified registration's direct
`get_validated` signing identity read and post-ping direct live-binding fence.
The Reviewer reproduced both exact compiled helpers waiting approximately 300ms
on the shared SQL mutex with no Tokio timer progress (helper-boundary, not endpoint
or E2E proof). Registration's synchronous Runtime Worker scan also requires a
network/authority boundary; ping previously used an unbounded blocking admission.

Fix `925bce2a` introduces whole owned DB stages for validated active signing
identity and the post-ping current binding fence. Runtime Worker scans and ping
use existing finite Host admission. A changed/revoked binding still produces
conflict rather than attesting the earlier connection; owner/key/CAS validation
and existing whole store transactions are retained.

The adjacent management audit also closes cancellation separation between SQL
commit and local Runtime publication. Registration and metadata updates now own
mutation, expectations/registry/subscription publication, and response observation
in one accepted Host workflow, with their SQL helper awaited on the independent
DB owner. Public Runtime resource/detail helpers admit ownership; the already-owned
workflows call explicit `_on_host` variants directly instead of reacquiring the
same lane. There is no Runtime/network await inside a SQL transaction or nested
Host admission. A cancelled waiter cannot strand a committed metadata change
without publishing the matching binding.

Launch options were another mixed Runtime summary/config helper placed on sole
DB. Its signed Runtime authorization could enqueue DB work behind its own waiting
parent. It now uses Host, preserving SQL/control progress and finite ownership.
The audit traced remaining signing validation/binding-fence calls: UI signing
operations already use DB, Runtime gate calls occur on Host, and removal already
has an owned Host workflow. DNS validation stays async; pure local registration
construction/notification stays in the accepted publication owner.

Permanent regressions cover both authority stages under a real separate SQLite
IMMEDIATE writer and shared-mutex busy observer: current-thread timer/control
progress, overload503, cancelled actual DB lease retention/cleanup, positive
validation and revoked binding409. An actual signed loopback ping is held while
SQL contention starts; after its reply the real handler's post-ping fence queues
without blocking Tokio or Host control. Another signed loopback scan proves SQL/
timer progress and retained cancelled finite Host admission. The metadata mutation
case cancels while its own SQL is waiting, then observes one durable update and
an equal published binding after actual Host/DB drain. Producers are stopped and
drained while the original fixture runtime remains alive.

Old identity, binding, scan and launch placements each fail their expected
contention/watchdog boundary; fixed code passes. Logs:
`target/t748-management-old-{identity,binding,scan,launch}-placement-red.log`,
`target/t748-management-final-restored-green.log` (16 boundaries), registration
policy11 and connection policy13. A zero-match launch-options exploration is not
counted as proof; its real held-summary case is in the 16 boundaries. The first
fixture teardown named an unavailable method and failed compilation; it was
corrected to the existing `shutdown_async`, not counted as a semantic test run.

Exact code `925bce2a405b6d3255887fba88867ab8d23882dc` passed root `cargo check` and
original default-parallel Server **969 passed / 0 failed / 1 existing ignored**,
11 binary tests; library duration **172.25 seconds**. Only three deliberate
executor panics appeared, with no residual test processes. Workdir default153 /
http-client156, fmt/diff/naming558/0/0 passed. Same isolated cache; evidence logs:
`target/t748-management-{root-check,full-suite,workdir-default,workdir-http}.log`.
Docs-only publication `6d8c49b7` changed no tested code. MR seq11 independently
confirmed those fixes and repeated the full validation, but requested changes for
cancelled trust-key revocation; it did not approve the complete snapshot.

## Trust-key revocation must own post-commit subscription termination

Seq11 major `T748-RUNTIME-REVOKE-CANCEL-PUBLICATION` reproduced the authenticated
public DELETE trust-key route waiting for an actual SQLite writer. Cancelling its
HTTP waiter did not cancel accepted SQL: revoke, binding-ID replacement and audit
committed, but waiter-owned expectation removal and broker unregister were lost.
The independent artifact used the normal public Server library's inner router,
not outer Workspace dispatch or live-stream/E2E. New signed requests still fail
the live binding fence; that does not shut down an already connected subscription.

Fix `1a065db0` gives the whole key-revoke outcome one accepted finite Host owner:
intact CAS/audit SQL on the independent DB lane, local expectation removal, broker
unregister/Shutdown, and response observation. Owner/scope/input preflight, typed
trust conflicts and current-binding error responses remain unchanged. Response
reads use the already-owned `_on_host` detail helper, with no nested Host admission.
Admission/unknown-outcome errors retain the normal typed availability mapping;
there is no transaction split, new queue budget or automatic mutation retry.
The adjacent key-mutation caller audit confirms registration and metadata already
own publication and this is the sole production key-revoke caller.

The permanent cancellation regression exercises the **real outer Workspace
dispatch**, owner browser cookie, generated DELETE route, whole store mutation and
broker. Projection producers stop before the write lock is acquired, so the
fixture-local busy observer identifies this actual revoke transaction, not startup
SQL. Timer/control progress before the watchdog releases SQLite. After waiter
cancellation, the real admitted RequestLease keeps deletion Pending. Releasing the
writer and draining actual Host/DB owners persists exactly one revoke audit,
changes binding ID, removes expectations and broker registration, and delivers
`Closed/ServerShutdown` to the downstream. The loopback endpoint holds a WebSocket
handshake; this proves broker termination, not an established upstream socket or
complete product E2E. Existing broker protocol tests cover socket replacement.

A separate full-Host route regression observes 503 before mutation and unchanged
binding, audit and publication. Restoring only the exact old revoke method fails
both permanent tests: the cancelled job leaves broker registration behind, and
under overload the old handler commits revoke before its response helper returns
503. Fixed code passes both without retry or reduced parallelism. Evidence:
`target/t748-revoke-final-old-publication-red.log`,
`target/t748-revoke-fixed-boundaries.log` (18), `target/t748-revoke-policy.log` (3),
`target/t748-revoke-binding-policy.log` (16).
An initial counter probe outside dispatch was invalid because dispatch replaces it
with the real RequestLease; it was corrected to observe actual deletion drain.
A test-only overload channel type-inference compile rejection was corrected with
an explicit unit channel. Neither setup error is counted as semantic evidence.

Exact code `1a065db0fc5d07e2dcc1b9dac2fcfc0607b9a23c` passed root `cargo check` and
the original default-parallel Server **971 passed / 0 failed / 1 existing ignored**,
11 binary tests; library duration **173.05 seconds**. Only three deliberate executor
panic diagnostics appeared, and no retained test/repro processes were observed.
Workdir default153/http-client156, fmt/diff and naming558/0/0 passed. All Cargo
checks/tests use the isolated candidate cache. Logs:
`target/t748-revoke-{root-check,full-suite,workdir-default,workdir-http,fmt,names}.log`.
The following documentation-only publication changes no tested code and needs a
fresh exact-source independent verdict before an approved handoff.

No latency benchmark, production incident measurement, target integration,
Ticket conclusion, or running-environment update is claimed. T743 has now
published its caller inventory/source; its owner will compose the T748 facade and
new callers, then obtain review of that exact combined source. T748 does not copy
those domain APIs or wait on the former unpublished branch.
