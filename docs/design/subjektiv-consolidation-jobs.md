# Backend subjektiv consolidation Jobs (T-704)

subjektiv requests the common Backend Job runner with an explicit
`builtin:subjektiv-memory-consolidation` Profile. The runner has no purpose-based
Profile switch. Subject-body Workers and the independent Workspace Memory feature
are not Jobs and are not migrated or reset here. The shared/standalone adapters
planned in T-708/T-709 are outside this Backend change.

## Input, grants and concurrency

The existing backlog/byte thresholds and missing/stale/failed surface policy
remain the launch decision. A Job captures an immutable bounded batch (up to 100
candidate IDs) plus the Subject store revision. An empty batch is valid for a
surface-only rebuild. The Backend explicitly binds the Subject and candidate
batch grant in durable Job intent before Worker execution. No subject-body
singleton ownership or Session attribution is transferred to the Job.

Authenticated `(Workspace, Runtime, Worker)` registry binding and the exact
current dispatched attempt authorize each operation. Names, Profile tags and
model-provided `subject_id` are not authority. Candidate listing exposes only
unresolved IDs in the captured batch; reads/decisions outside it are denied.
Surface prepare requires all batch candidates to have durable dispositions.
Surface generation records preserve immutable Job/attempt provenance (subjektiv
schema migration 6); publication, failure and completion reject another attempt's
generation, including pre-Job/legacy generations.

The common resource key `subjektiv-consolidation:<subject>` prevents parallel
Jobs for a Subject. Concurrent requests return the original active intent and
report `skipped_existing_job`; they do not expand its grants. New candidates and
items beyond the bounded batch stay in staging. A subsequent consolidation
request snapshots remaining work; it is not counted as processed by the first
Job. There is no domain-specific status ledger, pool or persistent idle Worker.
Definitive failures use the common bounded retry API with the original intent;
unknown execution stays fenced for attention. A later explicitly requested Job
may snapshot remaining candidates once all previous Workers are safely cleaned.

## Memory and surface completion

`MemoryApplyCandidate` still uses the existing expected-revision checks, atomic
apply/close and immutable decision receipts. Partial application survives model
failure, timeout, lost response, retry, Worker deletion and Backend recovery.
Job replay is not a second memory apply: resolved candidates retain their
existing dispositions, and exact candidate decisions converge on their receipts.

`SubmitBackendJobResult` for a bound consolidation Job receives only the Subject
and captured candidate IDs. Its Host capability awaits the existing bounded
clean-context surface editor and publish-or-recorded-failure, then adds the actual
structured surface outcome. It does **not** schedule a Worker-owned post-Run task.
The result call serializes and caches the exact generated outcome for ambiguous
submission replay. The Backend verifies every batch disposition and the current
store revision, generation ownership and persisted ready/failed surface fields;
it enriches the durable result with actual immutable candidate dispositions.
Thus confirmed-memory consolidation and surface readiness are separate outcomes.
A recorded surface failure does not undo Memory. A stale/unfinished/fabricated
surface cannot be reported completed. Final prose or Idle is never success.

Only after durable structured result acceptance does the existing terminal Job
cleanup run. Its tracked Run teardown fence lets the result tool finish before
stop/remove. Timeout/unknown paths cancel unfinished Worker execution through
that same cleanup; later operations/results are rejected. The next request can
regenerate failed/stale surfaces without touching confirmed memory.

## Legacy cutover and recovery

The subject-specific direct spawn and Idle resubmission paths are removed.
On a consolidation request the Backend checks the **persisted singleton owner**
for the old `subjektiv-consolidation:<subject>` key. It revalidates Workspace,
key and old dedicated recipe provenance, not the display name. A running/paused
legacy foreground operation is left alone and returns `skipped_legacy_inflight`;
no Job is launched alongside it. Legacy owned operations can finish during this
bounded cutover.

After foreground Idle (or Stopped) is confirmed, Backend stop is confirmed before
canonical WorkerRemovalService removal. Any legacy surface background task is
cancelled at stop; the subsequent Job rebuilds from confirmed Memory and does not
infer successful surface completion from Idle. Busy/unobservable/mismatched
owners fail closed. Empty/ready Subjects also retire their idle legacy owner.
Removal failures remain diagnosable through existing durable removal recovery;
a subsequent request rechecks ownership instead of deleting storage directly.
Subject-body/ordinary/Workspace-Memory Workers are never selected by this key.

The Job runner uses its existing durable dispatch allocation, tracked input,
T-701 registry-bound restore/recovery, deadlines, unknown outcomes, result replay,
delivery and T-690 cleanup. Job results, candidate receipts, Memory revisions,
provenance and surfaces remain in their existing Backend stores after Worker
removal. No live dogfood Worker or environment is stopped/updated by development
validation.

## Validation boundary

Server recording-backend fixtures prove reservation/binding/auth/result/cleanup
contracts, and existing store tests prove candidate CAS/atomic receipts and
surface freshness. Worker scripted-client tests prove the clean-context editor
completion-before-result boundary and result replay. They are not real-provider
LLM or live dogfood process evidence. See the T-704 validation report for exact
commands/results and inherited intermittent failures; later passes do not erase
those failures.
