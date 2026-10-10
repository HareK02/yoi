# Workspace request admission and deletion

Workspace HTTP dispatch uses `WorkspaceAdmission` for **lifecycle admission**, not a Workspace-wide operation mutex. Each admitted handler owns a drop-safe lease. Independent requests (including read-oriented POSTs) may execute concurrently; proof validation occurs before any business/resource wait in the inner router.

## Ordering and authority

1. Acquire an immediate lifecycle lease. While deletion drains, new ordinary requests fail closed instead of queueing behind the deletion operation.
2. Validate the bounded request body, Runtime signature, audience/Workspace/permission/method/target/body binding, expiry, transactional JTI consumption, current Runtime trust, Worker membership/reservation and singleton ownership. Browser actor/origin checks are unchanged. No TTL extension or 401 retry policy is introduced.
3. Check the persisted Workspace state. Scoped requests, including GETs, reject a deleting Workspace, including after Server reconstruction. An absent Workspace remains 404. Server-owned deletion status/polling is outside scoped dispatch.
4. Execute the inner router. Resource owners retain their existing exclusion: per-Worker session/control/removal locks; transactional Workdir attachment exclusivity and removal fences; SQLite domain transactions and revision/CAS checks; the resource broker's one-shot handle consumption. Legacy memory document read/replace/write uses a short, resource-specific mutex because it spans multiple store calls.
5. Drop the lifecycle lease on response, error, or cancellation. An admitted long operation is not rechecked for proof expiry merely because its business execution lasts past the proof TTL. This does not grant subsequent requests or disable provider/attachment authority checks.

The proof verifier's clock is injected into the Server instance for deterministic route tests, not obtained from test environment variables. Production uses the same wall-clock seconds and existing proof rules. Clock-skew policy remains a separate scope (T-728).

## Deletion and callbacks

Deletion start serializes only with other deletion starts. Before awaiting active requests it closes ordinary admission, so ongoing arrivals cannot starve the drain. The fence itself is RAII: a cancelled wait or a rejected preflight reopens admission. Successful reservation persists the deleting state before cleanup is scheduled; the persisted state remains the fence after the temporary drain lease is released.

While at least one ordinary handler remains admitted, these exact callback paths may obtain their own tracked leases:

- POST `/api/w/{workspace_id}/subjektiv/sessions` (create/restore session attribution)
- POST `/api/runtime/v1/workspaces/{workspace_id}/resources/fetch` (bound Runtime resource handles)
- GET `/api/w/{workspace_id}/runtime-config`
- GET `/api/w/{workspace_id}/worker-discovery/workers`

They do not bypass authentication or authorization. Callback admission closes atomically when the last ordinary lease leaves; deletion then waits for already-admitted callbacks too. A callback stream cannot keep adding work once all parents have finished. There is no deletion write-lock queue in front of callbacks required by a parent.

## Evidence boundary

`server/tests/workspace_admission_tests.rs` exercises the real outer dispatcher and generated inner routes with isolated SQLite stores, authenticated Runtime proofs and controlled external-provider fixtures. The long-operation fixture stops A at its provider response and proves B's ShowTicket/QueryTicket POSTs and separate Workdir read finish before releasing A. It also covers same-session exclusion, conflicting Workdir attachments, memory edit and JTI atomicity, drain/fence/cancellation, real create/restore with session callbacks overlapping deletion start, resource/config/discovery callbacks, injected-clock proof rejections and provider failures/disconnect/timeout recovery.

Timeout uses Tokio's test clock; proof tests advance an independent injected clock by 120 seconds. Bounds detect hangs, not guessed sleeps. Dispatch task cancellation represents cancellation of the HTTP handler future; provider disconnect is a dropped transport response. These are in-process boundary tests, not a real-socket disconnect or live browser/Runtime E2E test. Background command and websocket lifetimes retain their existing domain-specific cleanup; request admission covers the handler until its response, not the entire upgraded connection.

Past production `401 expired` incidents have **not** been correlated with live revision/request traces. The structural failure path and deterministic regression evidence do not establish that all prior 401s had this cause. T-731 and its retained session/MR/source/Workdir are unrelated and untouched. WIP remains 0.2.0; no generated DTO/schema/route artifacts change.
