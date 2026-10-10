# Workspace Drive storage

## Scope and ownership

Drive is a Workspace-owned **latest-version document tree**, not a mounted or
POSIX-compatible filesystem. `workspace-drive` owns storage, hierarchy, conflicting
changes, mutation receipts, bounded reads/search, and garbage collection.
`WorkspaceApi::drive` is the trusted Server adapter. The authorized HTTP API and
Backend-managed grants are documented below. Worker Tools/WIP, Web UI, and full
end-to-end routing belong to T-723–725.
Session attachments remain Session-owned and are not Drive authority. Worker,
Session, and Workdir cleanup do not delete Drive.

Only a single Server Host with persistent local disks is supported initially.
Multiple processes must use **the same metadata DB and blob root**. Separate
Hosts/independent DB copies may not write the same Drive. Workers on other Hosts
use the authorized API, not direct storage access. S3 deployment/migration,
POSIX/inode semantics, histories, mounts, and distributed quotas are not included.

## Configuration and persistent volumes

Server startup uses the existing resolved Yoi data directory. `ServerConfig` keeps
that resolved path in `data_root`; Drive code does not read environment variables
or introduce another default. At the default layout:

```
$YOI_DATA/drives/<internal-workspace-id>/blobs/<opaque-UUID>
$YOI_DATA/server/server.db
$YOI_DATA/server/feature-storage/<internal-workspace-id>/features/workspace-drive.sqlite
```

The FeatureStorage root is derived from the authority database path using the
existing Host convention; it is **not** placed below `drives`. Custom database
placements remain possible: configure the trusted `ServerConfig.data_root`
explicitly and persist both locations. Bind `$YOI_DATA` (and any custom database
root) to persistent volumes, not container writable layers, Worker checkouts,
or temporary directories. The storage identifier is the immutable internal
Workspace ID, never its display name or a user-supplied path. No blob key,
physical path, credentials, or SQL surface is included in node/read DTOs. Internal
storage errors contain diagnostics for Host logs; the authorized API adapter must
map them to sanitized failure responses, not expose raw error strings.

The directories and every ancestor are Host-owned and must not be writable by
untrusted actors. Existing symlink components, symlink blobs, non-directory
ancestors, nested directories, and path traversal are rejected. This is not a
sandbox against a hostile local administrator replacing paths concurrently:
`LocalFileSystem` uses path-based operations, not descriptor-relative no-follow
operations. External edits of the store are outside the Drive operation contract.

## Metadata and tree contract

Migration 1 registers `workspace-drive` through FeatureStorage. Each scoped DB
contains its Workspace binding, nodes, deletion tombstone, and mutation receipts.
SQLite `AUTOINCREMENT` node IDs are stable, scoped to a Workspace and never
reused after deletion. They are opaque application IDs, not inodes. Node IDs serialize as canonical positive **decimal strings**, preserving signed
64-bit precision across JavaScript DTO boundaries. `last_mutation_id` is the
existing request ID of the last committed mutation, not a counter or timestamp.
The immutable root has an empty value. Restore preserves `sqlite_sequence`,
node IDs, request IDs and receipts.

The DB is the only tree/latest-reference authority. Each node has one parent;
root is the sole parentless directory, has empty name, and cannot be renamed,
moved, deleted, or written. All other names are 1–255 UTF-8 bytes, compared
case-sensitively with SQLite binary equality. No Unicode normalization or case
folding is performed: canonically equivalent strings are distinct. Japanese is
supported. Control characters, `.`, `..`, slash and backslash are rejected. Names
never become blob paths. Same-parent names are protected by a unique DB index.
Parents must be directories in this scoped Workspace. Self/descendant moves are
rejected. Ordinary folder deletion is **empty-only**, never implicitly recursive.
Workspace destruction is the separate fenced purge operation.

Every update/move/delete requires `expected_mutation_id`. The same IMMEDIATE
transaction compares it with the node’s last successful request ID before writing.
A different committed request or
same-name/request-identity competitor returns `Conflict`; it never falls back to
unconditional writes. Root mutations and malformed requests are `Invalid`;
missing/deleted IDs are `NotFound`. No update counter is allocated or incremented.
Actor identity is trusted caller input (the authorization adapter supplies it),
and successful mutations record actor and UTC epoch milliseconds.

`BEGIN IMMEDIATE` is held for validation, blob operations, metadata mutation and
receipt insertion. It serializes the whole Workspace hierarchy and all blob
read/upload/GC activity across independent processes. A→B/B→A, parent deletion
versus child creation, and updates based on the same observation therefore have one valid winner.
FeatureStorage configures foreign keys, WAL, `synchronous=FULL`, and a 5-second
busy timeout. Busy/SQL/I/O failures remain errors; they do not mean an empty Drive.
Before opening SQLite or admitting a Feature handle, FeatureStorage creates and
syncs **every metadata directory ancestor link**, including configured path and
resolved target chains for trusted Host symlink paths. It re-syncs existing
directories on retry after any prior flush failure. Directory sync failure aborts
admission before DB initialization/migration; no usable connection is cached.
SQLite's DB/WAL and immediate-directory synchronization alone would not protect
new FeatureStorage/Workspace ancestors. Both this metadata link durability and the
separate blob link durability below are required for acknowledged Drive success.

The Server uses `Drive::open_with_workspace_authority` to attach its trusted
`server.db` to the Feature DB connection. An IMMEDIATE transaction also locks that
attached authority, and checks that the Workspace remains `active`. Consequently
a cached Drive handle cannot mutate after the deletion reservation commits, even
before the separate Drive tombstone is persisted. Drive never writes the
attached authority; only its metadata DB is changed. This conservative initial
implementation also serializes against other Server DB writes during blob I/O.
There is no cross-DB atomic write promise or distributed transaction: only the
Drive DB is written. Long uploads/searches can increase Server write contention;
all calls are blocking and async callers must use `spawn_blocking`.

## Blob publication and durability

Content creates/updates choose a fresh canonical UUID blob key unrelated to names
or parents. `object_store` **0.13.2**, with the `fs` feature and without cloud
features, supplies `local::LocalFileSystem`. `PutMode::Create` forbids overwrites.
Rename/move change metadata only; no blob move/copy/stat is needed. List/metadata
queries are SQL-only and still work if a referenced blob is missing, whereas
content reads diagnose the missing reference.

The pinned local implementation writes a `create_new` staging file, publishes
Create with `hard_link`, and best-effort removes staging. Inspection of 0.13.2's
local implementation found **no `with_fsync` option and no fsync** ([versioned
source](https://docs.rs/object_store/0.13.2/src/object_store/local.rs.html)).
The dependency is pinned to 0.13.2 because staging cleanup also depends on its
local filename protocol; reinspect publication/staging/durability before upgrading.
Atomic
publication is not power-loss durability. The small adapter compensates:

1. Create/validate Host directories; `sync_all` their entries and parents.
2. Use LocalFileSystem to atomically create the immutable blob.
3. `sync_all` the published file, then its containing directory.
4. Only after both succeed, update the DB reference/last committed request and insert the
   receipt in the **same SQL transaction**.
5. SQLite commit is the sole success/publication point.

Delete also syncs the directory, including on a missing-file retry. Sync, disk
full, permissions, and other errors propagate; metadata is not committed if blob
persistence fails. A failed sync may leave an unreferenced blob for GC. If DB
commit fails, the old reference remains and the new blob is likewise reclaimable.
There is no external-store/DB two-phase commit or bespoke object-store engine.

Supported durability target: **Linux, local persistent filesystem with working
file and directory fsync and SQLite WAL locking**. Filesystems/drivers that do not
support these calls fail rather than silently downgrade. Windows, network
filesystems, and other OS/storage combinations have not been qualified. Disk
controllers and devices must honor flushes. Unit tests verify ordering/failure
semantics with injected errors and actual isolated files/SQLite; they are **not a
power-cut experiment** and do not prove hardware power-loss behavior.

## Request result reconciliation

Every mutation includes a bounded request identity (1–128 bytes) and trusted
actor (1–256 bytes). A SHA-256 fingerprint covers the actor and typed mutation,
including content. Its committed result is stored with metadata in the same
transaction. Replaying the exact same request returns the recorded result;
reusing the ID with different input/actor is `Conflict`. Results are retained for
the Workspace lifetime, including results referring to later deleted nodes.
There is no clock-based deduplication window or partial FS-recovery engine.

After response loss or uncertain SQL/I/O errors, query `request_status`:
`Committed` includes the original result; `Uncommitted` means no committed row in
that authority snapshot. It is **not proof that an in-flight concurrent request
can never commit**. Retry with the same identity/payload, never manufacture a new
ID to resolve an unknown outcome. Failed/uncommitted operations have no success
receipt. A committed result can still be queried/replayed after the Drive is
fenced; it does not authorize new mutation.

## Bounds, pagination, reads and search

Hard limits are exported constants, not additional environment variables:

| Operation | Limit |
|---|---:|
| File create/update | 16 MiB (empty files supported) |
| Binary/text read chunk | 1 MiB, positive requested length |
| List page / GC page | 200 nodes / 200 blob keys + up to 200 staging files |
| Search work per page | 128 nodes |
| Text inspected per candidate | 64 KiB |
| Search query | 256 UTF-8 bytes |
| Name | 255 UTF-8 bytes |
| Content type | 128 ASCII bytes, media-type separator required |

No offset writes or shared write handles exist; whole content is replaced. Size,
invalid bounds and invalid UTF-8 errors are explicit `Invalid`; storage/capacity
errors remain failures for the API to present. There is no quota reservation.
Request and node/receipt volume remain bounded by available Host disk capacity;
operators must monitor free space for both blobs and DB/WAL. Files/receipts can
consume space until GC/Workspace deletion; there is no distributed quota system.

List uses ascending stable ID keyset pagination. Metadata is not a multi-call
snapshot: concurrent nodes may appear/disappear and relocation can change page
membership. Continuation is the last examined ID, not an offset or name. Search
uses the same non-snapshot ID semantics, examining at most its work budget per
page; an empty matching page can still have continuation. Name search is exact
substring matching. Optional bounded text search inspects only UTF-8 `text/*`
files no larger than 64 KiB, never images/large files. No index/embedding exists.

Each read supplies node ID **and the observed committed mutation**, and is locked to that
blob for the entire chunk. The SQLite lock excludes upload/GC while reading.
After an update the previous observation expires: the next chunk returns `Conflict` (or
`NotFound` after deletion), not the new generation. No long-lived read lease or
full-history retention is promised. Binary ranges are byte-based and exact;
text reads require UTF-8 character-aligned offsets/ranges and reject split or
invalid characters. Missing referenced blobs and length inconsistencies are
errors, including missing zero-byte files, never successful empty content.

## Garbage collection and restart

GC takes the **same SQL IMMEDIATE lock** as readers/uploads. Thus there is no
in-flight upload while sweeping; every currently referenced key is checked in
metadata before deletion. A grace period or process-local mutex is not a safety
proof. After a process crash SQLite releases/rolls back the transaction; an
unfinished staging file or fully saved but uncommitted blob can be reclaimed on
a later sweep. Recognized 0.13.2 staging names are cleaned under that lock; foreign
files are not interpreted as blobs. Deleted/replaced blobs are reclaimed too.

`collect` is explicit Host maintenance, not a background task tied to a Worker.
Repeat its cursor pages, then start future sweeps at `None` because new UUID keys
can sort before the preceding cursor. Local object listing necessarily traverses
the flat blob directory; response/deletion batches and candidate memory are
bounded, unlike SQL list which does no blob traversal. Holding the write lock for
local listing is a conservative safety/throughput tradeoff. A corrupt or redirected
storage layout fails closed. GC never deletes a key referenced by the DB.

## Workspace deletion

Deletion reservation in Server authority changes the Workspace to `deleting`.
The attached authority fence immediately rejects new mutations from cached Drive
handles. Deletion start also persists the Drive tombstone before responding or
scheduling cleanup; API restoration reapplies it for blocked/failed deletions.
Purge commits that fence first, removes only this Workspace's blobs/staging,
clears its nodes/receipts, then Host removes the blob directory and calls
FeatureStorage's scoped delete. The feature scope's admission fence prevents
cached tasks recreating DB files during finalization. Failures retain a retryable
Server deletion operation; repeating purge/fence/delete is safe. Other Workspace
roots are never traversed or removed. Root/tombstone remain in the domain DB until
the Host removes the scope; there is no normal client recursive folder deletion.

## Consistent backup and restore (offline initial procedure)

A FeatureStorage DB snapshot **alone is not a Drive backup**. No online
DB-plus-object snapshot is implemented. Use this administrative procedure:

1. Stop request admission, **all** Server processes sharing the authority,
   uploads/readers, maintenance/GC and deletion. Do not merely stop Workers.
   Complete or reconcile in-flight requests before backup.
2. Checkpoint/close the metadata DB using SQLite/FeatureStorage shutdown or use
   its SQLite backup API while all Drive writers and GC remain stopped. Never
   copy an active DB and omit its WAL. Capture the existing FeatureStorage scoped
   metadata tree (or its snapshot manifest + databases), Server authority and
   other deployment-required secrets/config using their respective procedures.
3. Copy the **same stopped generation's blob tree** with the metadata. Include
   all referenced immutable objects; copying unreferenced blobs/staging is safe
   and later GC can reclaim them. Preserve the internal Workspace ID. Record
   both roots, object_store/schema version and checksums in an operator manifest;
   restrict backup permissions because it contains user documents.
4. Verify each file node's blob exists and length matches `size`; missing content
   is a corrupt backup, not an empty file. Flush/sync the backup using the backup
   destination's own durability facilities before treating it as complete.
5. Restore only with admission/writers/GC still stopped, into absent FeatureStorage
   scope and Host blob roots. Use `ScopedFeatureStorage::restore` for its snapshot
   format, or restore the entire stopped deployment consistently. Never restore
   just `server.db` or create a new empty Drive DB over surviving blobs.
6. Restore blobs from the matching generation; restore internal IDs, committed request IDs,
   receipts and `sqlite_sequence` unchanged. Validate references/lengths and
   sanitized metadata/text/binary reads before reopening admission. Resume GC
   only after validation. A deletion tombstone must not be cleared on restore.

The hermetic backup test stops writers, snapshots actual FeatureStorage SQLite,
copies actual LocalFileSystem blobs, restores into absent roots, and checks stable
IDs/latest content/receipts and non-reused ID allocation. It does not validate a
production backup scheduler, volume snapshot, or physical power cut.

## Authorized typed HTTP API

The Rust `server-api` contract owns `/api/w/{workspace_id}/drive/` operations:

- `GET root`, `metadata`, `list`, `search`, `read-text`, `read-chunk`, `download`;
- `POST mutate` (create folder/bounded UTF-8 text, replace text, relocate, delete);
- `PUT upload` (binary body with flat query metadata);
- `GET requests/{request_id}` for the committed receipt snapshot;
- owner-controlled `POST/GET grants` and `DELETE grants/{grant_id}`.

Ordinary usage follows the existing member-facing authenticated Server-user
policy. Non-ownership alone does not deny use. Grant management follows the
existing Workspace-owner account boundary. No new general membership ACL is
introduced. Browser/API-token identity is supplied by authentication middleware;
user/account existence and active Workspace state are checked again in the
operation transaction. Cookie mutations require the configured application
Origin through the existing middleware. No ordinary DTO accepts an actor.

Runtime-forwarded requests require the existing signed source proof and live
Runtime/Worker catalog membership. Workers receive an explicit whole-Drive
`read_only` or `read_write` grant, independently of Profile, Subject, Ticket,
Workdir or commands. Incomplete/mismatched Runtime or Worker identity cannot fall
back to a browser actor. Grant creation rechecks owner, active Workspace, and
existing live Worker in the Server DB transaction; active access changes require
revoking the previous grant first (no silent downgrade). Migration 84 stores
creation/revocation actor and time. Removal fences and catalog deletion revoke
access and retain audit; grant rows cascade with Workspace deletion. No alternate
Worker registry or Drive Workdir attachment is created.

A clone-bound Drive authorizer queries the attached Server authority under every
client-operation `BEGIN IMMEDIATE`, including receipt replay/result queries.
The lock spans validation, blob persistence and DB publication. Grant revocation
and Worker removal use the same Server DB write authority: whichever transaction
wins first is ordered first. A mutation committing before revocation is valid;
a revoked cached handle cannot commit later. Upload admission also checks access
before consuming the body, but that admission is not a lease: publication always
rechecks current access. Download checks authorization separately for every
chunk, and stops on revocation, deletion, a changed committed request or storage failure.

References contain **both** Workspace ID and stable decimal node ID. JSON uses
`DriveEntryRef`; flat GET/binary queries require `entry_workspace_id` matching the
route Workspace before resolving `id`/`parent_id`. Node IDs are canonical
positive decimal strings within signed 64-bit range. Mutation IDs are request
strings (1–128 UTF-8 bytes, no control characters); file sizes/offsets are
bounded `u32`. List/search cursors carry Workspace identity and the last node ID;
they are continuation hints, not capabilities or snapshots. Returned `latest_url`
is a relative authenticated URL bound to Workspace and ID, independent of logical
name/parent. It confers neither public sharing nor approval. File URLs download;
folder URLs return metadata. Deletion/recreation never reuses the old reference.

Web upload consumes binary HTTP frames with an observed **16 MiB** ceiling and
checks declared size and lowercase SHA-256 before publication. Oversized announced
or received data, mismatches, aborted bodies and I/O errors never return success.
The storage boundary currently requires a whole-file Vec, so uploads retain at
most one bounded binary file in memory before calling T-721; they are not
constant-memory object-store multipart uploads and are never expanded to
JSON/base64. Runtime source-proof verification separately buffers a bounded body
before granting trusted ingress, as in other signed Server operations. Web
downloads stream **64 KiB** committed-request-bound chunks; they do not materialize the
whole file. Generated Rust clients retain their explicit bounded-response policy
and return exact `BinaryBody` bytes. Worker JSON-facing adapters can request
bounded text or binary chunks through this same contract, but Tools/WIP and
model-context/image presentation belong to T-723; this API does not emit large
base64 JSON blobs.

Downloads validate bare MIME types and always use `Content-Disposition:
attachment` with percent-encoded UTF-8 names, `nosniff`, a sandbox/default-none
CSP, `private, no-store`, and a SHA-256 ETag over Workspace, node ID and committed request ID. Arbitrary HTML/SVG
bytes are storable, not safe app-origin inline previews. Content length is exact;
if a later chunk loses authority or expires, the response stream fails rather
than returning another generation or a successful truncated file. Even an empty
file validates its referenced blob before HTTP success. Conditional 304/range
HTTP semantics are not added; observation-bound chunk reads are explicit.

Errors use fixed, path-free typed codes: `denied`, `not_found`, `conflict`,
`invalid`, `limit`, `storage_unavailable`, `outcome_unknown`. SQL/commit/task
uncertainty at a mutation boundary is `outcome_unknown`/`unknown`; known rejection
or blob persistence failure is `not_committed`. A successful DB commit alone
returns a mutation response. After a lost response, query the same request ID;
`uncommitted` only means no committed receipt in that snapshot and must not be
interpreted as proof that an in-flight request can never commit. Exact replay uses
the same identity/payload/transport-bound actor and current permissions. Never
blindly create a fresh request ID to resolve an unknown outcome. Physical paths,
blob keys and raw provider errors are not public.

Contract regeneration/freshness:

```sh
cargo run -q -p server-api --example export_openapi -- openapi/server-api.json
cargo run -q -p server-api --example export_openapi -- --check openapi/server-api.json
cargo run -q -p server-api --features typescript --example generate_drive_api_types > web/workspace/src/lib/generated/drive-api.ts
deno fmt web/workspace/src/lib/generated/drive-api.ts
cargo test -p server-api --features typescript generated_drive_api_contract_is_current
# Existing OpenAPI-derived projections embed the whole-contract digest:
for example in generate_repository_openapi_types generate_runtime_api_types generate_ticket_api_types generate_companion_api_types generate_worker_launch_api_types; do
    cargo run -q -p server-api --example "$example"
    cargo run -q -p server-api --example "$example" -- --check
done
```

The existing BinaryBody contract also supports binary **success responses**,
including declared response headers, without changing binary request bytes or
JSON/error behavior. It exports inline OpenAPI binary bodies, not JSON byte arrays.

## Upgrade to receipt-bound mutation checks

Migration 2 obtains each non-root node’s latest request ID from its existing
committed receipt, transforms historical receipt payloads, and drops the old
update counter. The current-state receipt must be unique; missing or ambiguous
evidence aborts and rolls back the entire migration. Equal timestamps do not
change the result. No content, node identity, request fingerprint or blob is
regenerated. Deleted nodes’ receipts and AUTOINCREMENT history remain intact.

The frozen migration module retains old SQL/JSON names and proven predecessor
correspondence only to verify existing receipts against their original request
fingerprints. This evidence never authorizes a new mutation. Active node CAS and
HTTP contracts do not accept the old numeric precondition. Exact pre-upgrade
Update/Relocate/Delete replays are accepted only when the complete actor/payload
and mapped predecessor reproduce the original fingerprint. If that evidence
cannot be established, recover the committed result through `request_status`;
changing the actor, payload or precondition and reusing its ID is a conflict,
not permission to execute it again.
