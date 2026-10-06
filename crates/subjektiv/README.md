# subjektiv

Shared Subject-scoped Memory domain and model-facing operations. This crate has
no Workspace Server dependency. `server-api` is the existing transport-neutral
request/response contract crate, not an authentication or Server implementation.

## Storage scope

The trusted Host creates `feature_storage::FeatureStorage`, selects
`FeatureStorage::scope(scope_id)`, registers `REGISTRATION` (or uses
`SubjektivStore::register`), and opens `SubjektivStore::open`. A trusted managed
`FeatureDatabase` can also be opened with `SubjektivStore::from_database`.

`scope_id` identifies the physical storage sharing boundary. Backend uses its
existing Workspace ID. Standalone uses a persisted random local storage-scope
identity under its operator-managed state directory, **not a fake Workspace**.
Subject IDs are independently issued random domain identities. An ID is a lookup
key, never an authorization credential.

Existing SQL schema and migrations are retained. In particular,
`store_scope.workspace_id` still stores the physical storage scope, under its
historical column name. Existing Backend placement and snapshot manifest keys
are unchanged. Optional historical `EvidenceOrigin.workspace_id` values continue
to be checked against the physical scope; local provenance need not invent a
Workspace value. `workspace_id()` is a Backend compatibility alias for
`scope_id()`.

## Host operation boundary

`api::execute(store, &HostOperationContext, operation)` is the common dispatcher
used by Backend and local Hosts. It owns query/cursor rules, bounded body and
nested provenance projections, immutable revision reads, proposal validation,
explicit staging/receipt rules, resident context, candidate decisions and
surface generation/publication/failure projections.

The Host context is not serializable or model input. Trusted Rust Hosts construct
it only after validating their external execution authority:

- `HostOperationContext::validated_body` binds one Subject and optionally an
  existing immutable `SubjectSessionAttribution`. Explicit staging requires the
  request Session to match that attribution.
- `HostOperationContext::validated_consolidation` binds one Subject, an optional
  immutable candidate batch, and an optional `JobAttemptBinding`. Attenuated Jobs
  must supply `Some(batch)`, including an empty batch; `None` is legacy non-Job
  Host consolidation authority. Body-only operations are unavailable to
  consolidation contexts, and candidate reads/decisions cannot escape the batch.

These constructors validate store/domain bindings, not external authentication.
Backend retains authenticated Runtime/Worker identity, current singleton and
live Job attempt checks, candidate grant checks, and public Session validation.
Local Hosts retain exclusive Subject execution leases, generic Job admission,
Worker/Session ownership, and committed public Session evidence validation.
Before explicit staging, the Host resolves evidence using its committed public
Session projection; the common API does not inspect Session files, hidden
reasoning, system prompts, or arbitrary paths. Lower-level repository/projection
functions are trusted Rust APIs, not model capabilities.

## Common consolidation Job rules

`job::bounded_candidate_batch(ids, max_result_bytes)` selects the exact T-704
ordered-prefix budget (100 candidates maximum, 2,048-byte envelope, twice each
JSON-escaped ID size plus 128 bytes per disposition).
`job::consolidation_required(count, bytes, force, availability)` owns the shared
trigger thresholds and empty-ready/surface-retry rules.

`job::validate_result(store, subject_id, batch, job_id, attempt_id, value)` checks
the strict result shape, exact admitted Subject/batch, durable dispositions, and
the persisted terminal surface outcome from that attempt. After validation,
`job::with_dispositions(store, subject_id, batch, value)` adds the deterministic
compact disposition projection. Hosts retain Job result binding/admission; these
helpers never run or replay a Job. The store's `prepare_job_surface_generation`,
`require_job_surface_generation`, and `validate_job_surface_outcome` are public
trusted-domain methods for both Backend and local Hosts.

Dependencies: `feature-storage`, `chrono`, `memory`, `manifest`, `rusqlite`,
`serde`, `serde_json`, `server-api`, `thiserror`, `uuid`; tests use `tempfile`.
