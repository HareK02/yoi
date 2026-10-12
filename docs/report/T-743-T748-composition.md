> Historical evidence: the user withdrew composition/wait instructions below.
> Current standalone correction and fresh validation/review evidence:
> `T-743-standalone-correction.md`. The original body is retained, not a T-748 return gate.

# T-743 / T-748 composition: execution-boundary evidence

## Source and status

T-743 continues on `work/T-743-ticket-public-surfaces`, MR
`01a12d19-b61b-79a2-baca-89241fd3c3d3` → `develop`. The published T-748
source `66bb9d0b825d72e20923003b273b787007f3aed8` was normally fetched and
merged into that same source branch, without conflicts, replacement selectors,
force push, or a copied execution facade. The composition commit carrying this
report also connects the added callers below. Its exact published hash is in the
Ticket handoff and provider-resolved MR source.

**This is a validated intermediate composition, not final independent approval.**
T-748 MR `01a12d3f-e90c-7b02-9ed9-cf3bd2e822a1` seq3 requests changes on this
upstream source. Its two major findings are not fixed by the new assignment
caller work: `T748-DB-NETWORK-LANE` (Subjektiv mixed Runtime/DB placement) and
`T748-ASYNC-PROTOCOL-SIGNING` (cold protocol observation source signing).
That authoritative verdict was checked again after the combined full suite;
no newer published upstream source or verdict was present. The T-748 owner was
informed through its Ticket of the composition and requested to provide the
normally advanced validated fix source. Do not interpret a green suite as proof
that those untested boundaries are safe. Normal-merge that fix into this same
T-743 source, revalidate, and independently review the exact combined source.

## Caller placement

- Canonical and Browser assignment pages and sanitized diagnostics share one
  implementation. Workspace scope, Ticket reference resolution and each whole
  synchronous Store read run on the shared bounded DB owner. None is left as a
  synchronous pre-call on a Tokio request thread.
- Typed Ticket query, including principal/handling/current-closure filtering
  before pagination, retains T-748's whole authority/read/enrichment DB job.
  The regression holds the **authority's actual candidate connection**, not the
  separate API Store connection used by only some enrichment paths. This also
  proves the empty-result path and wrong-principal filters offload correctly.
- Account assignment answers derive attribution from Backend-authenticated
  context inside the owned DB closure. Runtime answers preserve the live current
  Orchestrator discovery check on the finite Host lane, because Runtime worker
  lookup can perform remote HTTP. The DB closure derives Runtime attribution
  without I/O and the unchanged answer helper validates current singleton and
  live binding **again under its IMMEDIATE write transaction**, including replay.
  Target principal and request-body prose never supply author authority.
- The same answer helper keeps source authority, actionable-assignment gate,
  assignment/event CAS, full operation fingerprint/replay, body, audit, receipt
  and commit together. There is no await within that transaction and no automatic
  mutation retry.
- The accepted SQL job admits the existing finite Host attention continuation
  **after commit and before returning its result**. It inherits the request
  lifecycle source, owns lookup/Runtime dispatch independently of the waiter and
  does not wait on Runtime/network on the sole DB worker. A live HTTP waiter
  observes best-effort notification completion; losing the waiter does not
  cancel accepted SQL or accepted publication. Rejected Host publication retains
  the existing saved-input scan/recovery and diagnostic behavior, not rollback
  or a synthetic successful delivery.
- Normal Tools/WIP still converge on the canonical HTTP contracts. No second
  ledger, query adapter, source authority, read pool or delivery implementation
  was added. Existing T-748 set/clear/start/delivery boundaries remain intact.

## New regression proof

`crates/workspace-server/src/server_assignment_db_execution_tests.rs` contains
10 tests on fixture-owned bounded DB/Host owners, temporary databases and explicit
observable gates. Parent inspected the child's committed tool output, the actual
new test code and then the combined default-parallel suite, rather than treating
an end notification as proof. The child's delegated file scope was released.

The regressions prove:

1. Assignment/diagnostic read and Ticket reference resolution wait off the
   current-thread executor on their actual Store connection mutex.
2. Typed principal/handling/closure query matrices, including empty results,
   hold the candidate connection and filter before the page limit.
3. A real second SQLite connection's IMMEDIATE writer reaches the answer's owned
   busy observer while the same Tokio runtime's timer and independent control
   progress; one transaction preserves the answer/event/audit/receipt.
4. Saturated DB admission returns HTTP 503 and does not accept an answer.
5. Cancelling an accepted answer waiter retains its actual job slot/lifecycle
   through SQL and Host publication; body/audit/receipt persist, dispatch is once,
   and actual cleanup releases all observed leases.
6. Successful HTTP answer waits for its admitted publication without occupying
   the DB worker; an independent DB callback can progress during publication.
7. Account and Browser receipt replay, stale CAS, operation mismatch and bad
   cursor/error mapping retain their domain meanings.
8. Real signed non-Orchestrator source and signed proof replay are denied with
   no answer/receipt; signed current Orchestrator canonical/Browser replay shares
   one source-bound receipt, and another source cannot replay it.
9. Saturated **2-worker/16-queued Host** live-discovery rejects canonical/Browser
   Runtime answers before SQL admission, without receipt/audit/publication or
   retained lifecycle, while independent DB callbacks progress. This last test
   injects authenticated context at generated handlers to isolate discovery
   admission; the preceding tests exercise actual signed-proof middleware.

Read/query contention intentionally uses the actual connection mutex; answer
contention uses a separate real SQLite writer. Neither is described as a full
remote Runtime latency benchmark. No fixed completion sleeps, mutation retry,
parallelism reduction, process-global fixture budgets or new ignored tests were
introduced. Fixtures drain accepted Host/cleanup jobs, shut down projection
producers, then drain DB jobs before ending their original Tokio driver. Owned
busy-handler userdata and SQLite locks unregister/release through RAII.

## Validation on this composition

- Root `cargo check`: passed, normal default-member compile closure.
- `cargo test -p yoi-workspace-server --lib assignment_db_execution_tests`:
  **10 passed, 0 failed**, 2.63s (child committed output). Parent reran the first
  nine after separating Runtime discovery and included all ten in the full suite.
- `cargo test -p yoi-workspace-server`: original default parallelism,
  **985 library passed, 0 failed, 1 pre-existing ignored**, 176.60s;
  **11 binary passed**, doc tests passed. Log:
  `target/t743-combined-server-suite.log`.
- `cargo test -p workdir`: **153 passed** across library/integration targets;
  `cargo test -p workdir --features http-client`: **156 passed**, doc tests passed.
  Logs: `target/t743-combined-workdir.log`, `target/t743-combined-workdir-http.log`.
- Combined Web check: **0 errors, 0 warnings**; static production build passed
  with the existing chunk-size advisory. Web tests **613 passed**, components
  **318 passed**, Chromium Ticket/principal browser tests **7 passed**.
  Logs: `target/t743-combined-{web-check,web-build,web-tests,components,browser}.log`.
  The last change is Server execution placement/test code, not a UI change.
- All six server-api TypeScript generator `--check` commands passed: ticket,
  repository OpenAPI, runtime, workdir, Worker launch and companion.
- `cargo fmt --all -- --check`, `git diff --check HEAD` and
  `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts`
  passed; naming: **558 matching lines, 0 unapproved, 0 stale exceptions**.

Earlier T-743 public-surface, five-crate, Prompt/Tool, visual and real-Worker
fixture evidence is retained in `T-743-public-surface-validation.md`. Scripted
model/loopback browser fixtures do not attest real model judgment or all T-744
live paths. Target integration, Ticket/MR conclusion, live update and retained
Worker/Workdir cleanup were not performed. Fresh independent combined-source
review remains pending the upstream fixes, not silently waived.
