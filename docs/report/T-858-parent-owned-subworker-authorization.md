# T-858: Parent-owned Internal SubWorker authorization

## Change

Internal SubWorker creation and direct-child control belong to the parent Worker registry. They do not create an independently authorized Server principal.

- Removed `subworker.spawn/send_input/stop/observe` operation declarations and calls, the bound-check child target, the permission resource variant and its Server handling. Unknown old child authorization payloads are rejected; no replacement grant or compatibility authorization path was introduced.
- Spawn still reserves names, validates parent scope and capabilities, acquires write leases, constructs the child with its scoped broker, and performs existing failure cleanup. Independent Runtime Worker creation and Reviewer-specific MR capability registration remain separate, protected operations.
- Direct-child list, permission checks and stop use the parent registry. Other parents, unknown names and unsupported operations remain inaccessible. Workspace session observation never forwards an internal child selector; the local registry capture provider owns it.
- `AuthorizedWorkdirSession` forwards structural path/symlink validation and write-scope overlap queries to its underlying provider without treating an individual Host point authorization as a scope-delegation ceiling. Those checks do not return filesystem data or grant effects. Remote/External providers may still receive these provider-neutral structural queries; this is not a claim that spawn makes no network requests at all.
- Read, ReadBytes, Stat, Write, Edit, traversal and command execution retain their existing live Host checks. Child scopes, ancestor/provider limits, capability checks and write-lease conflicts remain in place.
- Added shutdown reconciliation for revoked command-output authority: cancel the owned command, then use status-only confirmation of termination if output access is denied. Do not read forbidden output or fabricate a transport terminal. Status errors, unknown commands in this fallback, or a ten-second unsettled wait retain the cleanup/write-lease fence for retry. Parent/sibling commands are not cancelled.

## Regression evidence

The request-only Host fixture reproduces the current core-grant response shape rather than always returning a broad matching scope. Tests cover recursive mixed read-root/write-subtree scopes, nonroot cwd, actual read/edit/write/search, nested attenuation, command denial, multiple attachment identity checks, write overlap and symlink alias conflicts, revocation, stopped-child access, and command cleanup without cancelling the parent.

The scripted SubWorker environment tests exercise production spawn, Worker construction, tool registration and routed reads with Local providers, including multiple aliases and nested children. They now check that local creation does not grant filesystem access when the parent's Host authority is revoked. A bound Workspace fixture panics on any unexpected Server request during ordinary internal spawn.

Two Server tests use real signed request routing and permission storage with isolated fixture Runtime identities:

- `signed_host_keys_and_provider_scope_guard_real_local_read_write_and_revocation_before_effects`: now partitions an internal writable scope over the live authorized Local provider, checks access, then revokes the parent policy and verifies child access fails before effects.
- `bound_authorization_rechecks_subject_and_rejects_internal_child_targets`: Subject-bound checks still work and are revoked normally; internal-child targets are no longer accepted.

## Validation

Against `hare/develop` based on `78e2ec952`:

| Command / filter | Result |
| --- | --- |
| `cargo test -p worker --lib authorized_workdir::tests` | 21 passed |
| `cargo test -p worker --lib spawn::` | 58 passed |
| `cargo test -p worker --lib feature::builtin::manage_worker::` | 36 passed |
| `cargo test -p worker --lib feature::builtin::worker_observation::` | 2 passed |
| `cargo test -p worker --lib builtin_catalog::` | 11 passed |
| `cargo test -p workdir --features http-client` | 198 passed across lib and integration suites |
| `cargo test -p server-api --lib` | 160 passed |
| The two Server tests named above | 2 passed |
| `cargo check --workspace --tests` | Passed, existing warnings remain |
| `deno task check` in `web/workspace` | 0 errors, 0 warnings |

OpenAPI, memory API TypeScript and the five OpenAPI-derived TypeScript artifacts were regenerated with the repository generators. Initial artifact tests correctly failed until those artifacts and their input digests were refreshed; the final Server API suite passed. The initial command-cleanup regression exposed the revoked-output problem and passed after the status-only reconciliation fix.

## Scope of evidence

These are component, signed-Host and scripted connection tests, not a live-model production deployment evaluation. The Workdir suite includes HTTP transport and remote cleanup coverage, but no new full production Server → External provider → model-run SubWorker deployment was exercised. The running Server/Runtime and `yoi-data` were not changed or restarted. Deployment and a retry of the original T-854 SubWorker spawn remain unverified.
