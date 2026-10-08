# Workspace Drive storage

## Scope and ownership

Drive is a Workspace-owned **latest-version document tree**, not a mounted or
POSIX-compatible filesystem. `workspace-drive` owns storage, hierarchy, revision
conflicts, mutation receipts, bounded reads/search, and garbage collection.
`WorkspaceApi::drive` is the trusted Server adapter. Authorization and HTTP DTOs
belong to T-722; Worker/Web clients and end-to-end routing belong to T-723–725.
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
reused after deletion. They are opaque application IDs, not inodes. IDs and
revisions serialize as canonical positive **decimal strings**, preserving signed
64-bit precision across JavaScript DTO boundaries. Restore preserves
`sqlite_sequence` as well as IDs and revisions.

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

Every update/move/delete requires `expected_revision`. A stale revision or
same-name/request-identity competitor returns `Conflict`; it never falls back to
unconditional writes. Root mutations and malformed requests are `Invalid`;
missing/deleted IDs are `NotFound`. Revision exhaustion fails rather than wrapping.
Actor identity is trusted caller input (the authorization adapter supplies it),
and successful mutations record actor and UTC epoch milliseconds.

`BEGIN IMMEDIATE` is held for validation, blob operations, metadata mutation and
receipt insertion. It serializes the whole Workspace hierarchy and all blob
read/upload/GC activity across independent processes. A→B/B→A, parent deletion
versus child creation, and same-revision updates therefore have one valid winner.
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
4. Only after both succeed, update the DB reference/revision and insert the
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

Each read supplies node ID **and the selected revision**, and is locked to that
blob for the entire chunk. The SQLite lock excludes upload/GC while reading.
After an update the old revision expires: the next chunk returns `Conflict` (or
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
6. Restore blobs from the matching generation; restore internal IDs, revisions,
   receipts and `sqlite_sequence` unchanged. Validate references/lengths and
   sanitized metadata/text/binary reads before reopening admission. Resume GC
   only after validation. A deletion tombstone must not be cleared on restore.

The hermetic backup test stops writers, snapshots actual FeatureStorage SQLite,
copies actual LocalFileSystem blobs, restores into absent roots, and checks stable
IDs/latest content/receipts and non-reused ID allocation. It does not validate a
production backup scheduler, volume snapshot, or physical power cut.
