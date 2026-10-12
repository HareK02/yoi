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

Final code source `49bfbfb2` passed the full default-parallel Server suite,
including the command-overload case: 960 passed, zero failures, one existing
ignored migration fixture; 11 binary tests, library duration 175.29 seconds.
The only captured panics were the three deliberate unknown-outcome tests.
Root `cargo check` compile closure passed (including TUI/Server), as did Workdir
default (153 tests) and `http-client` (156 tests) full suites, format/diff checks
and state-contract names (558 matches, zero unapproved, zero stale exceptions).
These commands used the isolated candidate artifact directory; libtest default
parallelism and the production budgets were unchanged. The MR records the exact
published source, which also includes this report and the boundary document.

No latency benchmark, production incident measurement, target integration,
Ticket conclusion, or running-environment update is claimed. T743 has now
published its caller inventory/source; its owner will compose the T748 facade and
new callers, then obtain review of that exact combined source. T748 does not copy
those domain APIs or wait on the former unpublished branch.
