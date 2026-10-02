# subjektiv product cutover runbook

This runbook defines the product and operator boundary for moving a Workspace from legacy single-document Memory to subject-scoped subjektiv Memory. The cutover changes which product path is active; it does not reinterpret legacy tool or API inputs, migrate legacy records into subjektiv, or make historical Worker attribution into current authority.

T-672 documents and validates the contract only. It does **not** authorize or perform a reset in any real environment.

## Authorities and persistence

Two independent Memory systems exist during the transition:

| Product path | Persistence owner | Durable content | Status |
| --- | --- | --- | --- |
| Legacy Workspace Memory | Workspace Server control-plane database | one Workspace document, staging candidates, and staging resolutions | legacy/deprecated; retained only for compatibility and explicit reset |
| Subject Memory | Server-managed `FeatureStorage`, in the Workspace-scoped `subjektiv` Feature database | subjects, immutable candidate and resolution records, confirmed Memory revisions, Session attribution, surface generation records, and immutable surface snapshots | current product path |

The shared `memory` crate and Worker Features do not own either database. `FeatureStorage` owns the subjektiv SQLite lifecycle (path, connection, migration, backup/restore, shutdown, and Workspace deletion), while the subjektiv repository owns its schema and domain transactions. The legacy control-plane tables are not imported or dual-written into subjektiv.

Every subject operation is bounded first by the authenticated Workspace and then by an explicitly authorized subject. A subject ID is a selector, not a credential. The Host derives subject scope from authenticated Runtime/Worker source proof and current ownership of the opaque `subjektiv:<subject-id>` Worker key; model-visible requests do not supply or expand that authority.

The current Worker remains Yoi's keyed-singleton responsibility. The subjektiv database records immutable historical Worker and Session attribution, but it contains no second `current_worker_id` mapping and historical attribution is never used to choose or authorize the current Worker. Stopping or replacing a Worker does not retire its subject, and retiring a subject does not stop a Worker.

Workspace Memory settings remain Workspace-owned configuration. When `feature.subjektiv` is enabled, the Backend binds the trusted settings snapshot used for extraction language; the snapshot is not subject identity and cannot be authored by a Profile, Browser, or model.

## Session, Memory, and correction semantics

- **Session attribution and recall:** only a committed primary-Worker Session with Host-recorded subject attribution enters subject Session discovery. Attribution is immutable and idempotent. Recall reads the retained or committed-archive Session projection under current Workspace, subject, observation, and retention authorization; it does not copy Session bodies into subjektiv, infer attribution for older Sessions, restore a Worker, or turn recalled tool output into `HumanInput`.
- **`SubjektivMemoryRemember`:** stages a new candidate; it does not directly create confirmed Memory. Explicit committed evidence is resolved against the Session capture. A non-preference request without an entry is staged only after its tool call commits, using the committed call as model-origin evidence. An uncommitted operation creates no candidate.
- **`SubjektivMemoryProposeRevision`:** stages a candidate with typed target metadata: exact Memory ID, expected revision, intent, and change reason. It does not mutate the target or silently retarget to a newer revision.
- **Candidate consolidation:** a restricted, subject-bound consolidator compares an immutable pending candidate with confirmed Memory and makes one idempotent atomic decision. An applied decision writes the new confirmed revision, exact provenance and affected revision references, and the immutable candidate resolution in one subjektiv transaction. Non-applied dispositions are explicit (`discarded`, `invalid`, `duplicate`, or `already_covered`). A failed or empty model run is not a disposition, and surface generation failure cannot undo an already committed decision.
- **Corrections:** refinement or correction of the same experience appends the next revision under the same Memory ID using an expected-revision check. Stale proposals remain unresolved/conflicted and are never rebased automatically. Retraction is terminal; a later corrected experience receives a new Memory ID and may derive from the old fixed revision. Old revisions, candidates, resolutions, provenance, and surface snapshots remain immutable history.

Confirmed Memory is useful subject context, not authorization and not exact implementation authority. Tickets, Objectives, repository state, Git history, and append-only Session records remain the exact sources for implementation state.

## Resident surface and history

A subject surface is a bounded rendering of current active confirmed revisions, not another Memory authority. The injection contract is:

- **ready:** inject the current non-empty snapshot only when it was built from the subject's exact current store revision;
- **ready-empty:** explicitly record that a successful current generation produced no Markdown and inject no Memory prose; do not conflate this with missing or failed generation;
- **ungenerated:** inject nothing and report no current surface;
- **stale:** inject nothing until regeneration publishes a snapshot for the current store revision;
- **failed:** inject nothing; retain the failure for diagnosis and retry rather than presenting it as an empty Memory.

A new Worker receives the current ready or ready-empty result in its initial durable prompt construction. A restored Worker does not rewrite its persisted prompt or prior Session history. It appends one durable restore-boundary system item that supersedes prior resident summaries with the latest ready surface, explicit ready-empty state, or a no-current-surface tombstone. Later turns in the same restored process do not append the boundary again. This preserves append-only history: no stale summary, failed generation, correction, or cutover operation edits previously committed Session entries or immutable subjektiv records.

## Legacy/deprecated compatibility boundary

The existing legacy tools (`MemoryQuery`, `MemoryReadDocument`, `MemoryUpdateDocument`, and the legacy `MemoryStaging*` operations) and `/api/w/{workspace_id}/memory...` APIs retain their established Workspace-document meanings while they exist. They are legacy/deprecated surfaces, not aliases for subject Memory. Do not silently route an old request to subjektiv, infer a subject from old input, or change an old input field's meaning.

New product work uses the explicit subject/session/confirmed-Memory/revision/source and surface operations. Old clients must be stopped or upgraded before reset; a post-reset legacy read may observe the legacy empty/default state, but it must never expose subjektiv records through the old schema.

## Explicit legacy reset contract

The legacy reset is a destructive, operator-triggered API operation scoped to one authorized Workspace. Startup, Profile resolution, enabling `feature.subjektiv`, Worker restore, consolidation, and ordinary reads must never trigger it implicitly.

In one control-plane database transaction, the reset deletes all of the following legacy rows for the Workspace:

1. the legacy Memory document;
2. all legacy Memory staging candidates;
3. all legacy Memory staging resolutions.

The operation is all-or-nothing. A failure before commit leaves all three legacy sets unchanged. A retry after success is idempotent: absence of those rows is success and must not delete anything outside the legacy set.

Legacy `AppendAudit` was acknowledgement-only and created no dedicated durable Memory-audit rows, so there is no fourth legacy audit table to erase. General Session logs and diagnostics remain retained under their own policies.

The reset must retain:

- Workspace Memory settings and their revision;
- Sessions, Session logs, archives, and diagnostics;
- Tickets, Objectives, and their audit/history;
- Knowledge and Skills;
- the entire Workspace-scoped subjektiv Feature database, including subjects, candidate/resolution history, confirmed revisions, attribution, generation runs, failures, and surface snapshots;
- all other new-product and unrelated Workspace data.

The reset is not a subject deletion API, a Workspace deletion API, a settings reset, a retention override, or a data migration.

## Operator procedure

Perform the cutover in this order:

1. **Stop legacy activity.** Disable legacy `feature.memory` Profiles and automatic extraction/consolidation, stop or drain old Workers, and block Browser/API clients from writing `/memory` routes. Wait for in-flight legacy writes to finish. Do not run legacy and new automatic extraction for the same Worker.
2. **Back up while writers are stopped.** Capture the control-plane database containing legacy Memory and a `FeatureStorage` backup containing subjektiv. Record the Workspace and backup identities and verify the backup artifacts. Feature backup is internally consistent, but there is no cross-database snapshot instant; stopping both writer sets is what makes the pair a usable cutover checkpoint.
3. **Trigger reset explicitly.** An authenticated operator invokes the legacy reset API for the intended Workspace. Do not automate this as a startup migration or infer consent from `feature.subjektiv` enablement.
4. **Verify both sides.** Confirm the legacy document, staging, and resolutions are absent (or return their defined empty/default projection); confirm Workspace Memory settings and unrelated control-plane data remain; reopen/read the subjektiv store and verify the expected subject, confirmed revisions, attribution, and current surface state are unchanged. Verify a subject Worker resolves through the `subjektiv:<subject-id>` singleton and has only the intended new tools.
5. **Resume only the new path.** Start subject Workers with `feature.subjektiv`, keep legacy write surfaces disabled, and monitor typed surface/Session diagnostics. Do not treat an ungenerated, stale, or failed surface as ready-empty.

### Failure and recovery

- If reset returns a definite failure, the transaction rollback means legacy document, staging, and resolutions remain together. Correct the cause and rerun the same explicit operation.
- If communication is lost and the result is ambiguous, keep legacy writers stopped, inspect all three legacy sets, and rerun. Idempotency makes a committed first attempt safe to repeat and atomicity prevents a valid partial state.
- If verification of retained data or the new path fails, keep both write paths stopped. Restore the coordinated pre-reset backups according to the normal control-plane and `FeatureStorage` restore procedures, verify them, and only then resume. Never "recover" by deleting or recreating the subjektiv database.
- Preserve Session logs and diagnostics throughout; they are recovery evidence, not reset targets.

## Validation boundary

The validation supporting this cutover proves substrate-level store shutdown/reopen behavior and restoration of a Worker object with its persisted history. It does **not** claim a restart of the Server or Runtime process, behavior during communication loss, real-model consolidation or surface quality, or a full-process end-to-end cutover. Those require separate operational validation before a production reset.

No real environment reset is performed or authorized by T-672.
