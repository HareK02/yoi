# Workdir refusal diagnostics

This change improves the internal `api_error` record for
`POST /api/w/{workspace_id}/workers/self/workdir-session/operations`.
It does **not** identify or resolve the reported 2026-10-09 403: the running
binary revision and actual request are still unverified. Fixture success is
not evidence about that incident. No deployment, live asset cleanup, privilege
change, retry policy, or WIP dependency/version change is included (WIP remains
0.2.0).

## Diagnostic path and public contract

Original refusing branches attach a closed `WorkdirDenialReason`, rather than
recovering meaning from an error string. `WorkdirError::Denied` carries local
message plus optional typed reason. `DenialContext` retains an existing error
classification while enriching its diagnostics (notably checkout's historical
permission-to-out-of-scope mapping). Typed provider markers survive the
`FsAccessPolicy`/`io::Error` → `FsError` → `WorkdirError` boundaries.

`WorkdirTransportError.denial_reason` and
`ExternalWorkdirOperationError.denial_reason` carry only an optional snake-case
enum. Remote HTTP and External errors preserve that reason when converted back
to `WorkdirError` and then forwarded again. Provider text is not the reason.
The External codec continues dropping provider-authored message text. A missing
reason stays unknown for generic `denied`; existing code-level path/symlink/
read-only classifications can still identify their original category. Unknown
or oversized reason strings fail deserialization rather than becoming log data.

Runtime registry session-open failures now retain a safe transport error **before**
the former raw error-to-string conversion. Their existing Workspace HTTP meaning
is still 502 (`workdir_session_open_failed`); operation-level permission failures
remain 403. Session-open messages are normalized to the same safe provider
message vocabulary instead of forwarding raw local errors. No authorization,
scope, capability, session ownership, command ownership, or retry conditions
are relaxed.

The self handler attaches internal context to its error. Both the handwritten
response adapter and the generated route preserve it through the typed error
response extension to `ApiErrorLog` and the logging middleware. Server API
`workdir_log` and `denial_reason` are `serde(skip)` and omitted from schemas:
the external error envelope remains `{code, message}` for provider failures or
the existing Repository-style envelope for API/domain failures. Workspace
responses do not expose the new internal diagnostics. Authorized Runtime/provider
transport carries the enum to its consumer; it contains no free text or paths.

## Fields in `yoi::api` / `api_error`

- `operation`: fixed label from the typed operation (read, read_bytes, write,
  edit, command_start/status/output/cancel, authorize_scope, scope_rules_overlap,
  checkout_search/observe/execute, stat/list/glob/grep). No operation Debug dump.
- `workdir_stage`: workspace_scope, worker_identity, attachment_validation,
  session_open, command_lookup, or provider_dispatch. API failures identify the
  refusing stage without copying raw alias/handle/error text into the log.
- `denial_reason`: optional closed reason reported by the provider/error chain.
  It describes the encountered refusing branch, not a permission grant and not
  an independent attestation of an external provider's explanation.
- `workspace_id_hash`, `runtime_id_hash`, `worker_id_hash`, `workdir_id_hash`,
  `attachment_alias_hash`: 64-character lowercase SHA-256 fingerprints via
  `worker_source::diagnostic_id_hash`, using the existing
  `yoi-runtime-proof-diagnostic-id-v1\0` domain prefix. Hash a known catalog ID
  or attachment alias to locate its logs. Workspace and alias represent route/
  request selection; Runtime/Worker are added only after current identity checks,
  and Workdir only after a validated attachment is found. Missing information is
  omitted, never inferred from a failed lookup or command handle.
- `request_token_id_hash`: fingerprint of the **existing verified Runtime proof
  JTI**, reusing the T-729 hash algorithm. It is not the proof, raw token ID,
  caller-supplied trace header, or a newly invented cross-service trace ID.
  In-process/test callers without that proof correlation omit it. Authentication
  still consumes the JTI using the existing policy; logging does not retry it.
- Existing `method`, query-free `path`, `status`, `kind`, `message`, `diagnostics`
  remain. Workdir-context method/path are bounded to 32/512 UTF-8 bytes with
  controls removed. Provider operation messages remain normalized safe text.
  Contextual API failures use a fixed internal message and status-based kind,
  with no raw domain diagnostics copied into this log. Empty `diagnostics` does
  **not** mean the optional typed reason/context is absent.

Fingerprints are bounded correlation, not encryption: low-entropy identifiers
can be guessed. Do not put credentials in IDs. Neither hashes nor stage labels
prove the reported incident's cause. The request JTI identifies the incoming
request; External operation IDs/generation/audit records remain their existing
provider-side mechanisms, not a cross-service trace automatically joined here.

## Refusal vocabulary and limits

Reasons distinguish scope resolution/comparison unavailable, attenuated
capability refusal, logical/parent/attachment/delegation scope exceeded,
empty scope, invalid scoped path, writable-scope requirements for commands,
child write-lease conflicts, unreadable cwd, read-only session, external root
symlink/change/move, external scope symlink, checkout output-root escape,
provider root escape, checkout search/enumeration refusal, provider symlink/
checked-target refusal, path out of scope, symlink target out of scope, and
read-only path. Original local messages remain local; arbitrary message text
is never parsed into a reason.

- `os_permission_denied`: PermissionDenied with an actual OS error code, without
  a recognized policy marker. It does not determine whether modes, ACLs,
  sandboxing, mount policy, or another OS mechanism caused the refusal.
- `unclassified_permission_denied`: custom PermissionDenied with neither typed
  policy origin nor raw OS code. Do not label an arbitrary
  `Error::new(PermissionDenied, "...")` as an OS refusal by guessing from text.
- External/remote providers that do not send a reason cannot supply distinctions
  erased in their own binary. A registered provider's typed explanation is
  reported data, not proof of what happened inside that provider.
- Authentication/JSON ingress failures before a typed Workdir operation and
  target can be obtained continue using their existing rejection diagnostics;
  no operation/body is guessed or decoded from rejected proof data.

## Regression proof boundaries

- Workdir provider fixtures exercise actual local authorization, broker scope/
  capability/read-only/delegation/lease refusals and confinement. Permission
  fixtures distinguish typed policy origins, OS-coded PermissionDenied, and
  origin-unknown custom errors, including checkout's existing public mapping.
- Loopback HTTP fake exercises remote open/operation decoding and typed reason
  forwarding, including missing-reason providers and hostile provider text.
  External JSON/frame codecs exercise the bounded error envelope and reject
  arbitrary reason strings; the Server's External adapter drives the real self
  generated route through command-session dispatch and response/log conversion.
- Server router tests use signed fixture requests and capture actual tracing
  events. They cover read/write/command_start/authorize_scope, session-open
  failures, External command status failures, no-identity rejection, oversized
  UTF-8 alias, oversized command/file/tool-call inputs, query/header/proof/key/
  credential markers, and raw provider/host paths. Identity hashes and the
  existing verified JTI fingerprint are checked against fixture identities.
- The Server API boundary proves internal fields are absent from body/schema.
  Worker HTTP→WIP coverage verifies diagnostic wrappers do not change refusal
  classification or induce retries; tools retain established checkout codes.

These are hermetic Rust/API/protocol fixtures, not a full product-process E2E,
not a deployed binary test, and not reproduction of the user's asset/request.
Validation commands and concrete counts are recorded on the implementation MR.
