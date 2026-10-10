# Worker Drive tools and WIP

Drive is Workspace-owned DB hierarchy and local blob storage exposed through the
authorized Server API. It is **not** a Workdir, mount, Git checkout, or a Session
attachment store. Worker code never accesses the Server's physical blob root.

## Enablement and authority

Set `feature.drive = { enabled = true; };` in a DCDL Profile (or
`[feature.drive] enabled = true` in TOML). Only activation is accepted; Profile
fields cannot specify Workspace, grant, actor, access, or host paths. Default,
Coder, Companion, Intake, Orchestrator and Ticket Worker builtin Profiles select
the feature. Standalone, Reviewer and Job defaults do not. Host must inject an
available Workspace-bound `WorkspaceClient`; standalone activation is inert.

An authorized Workspace owner must independently grant the **registered Runtime
Worker** whole-Drive `read_only` or `read_write` access through the T-722 grant API.
Every read, metadata resolution, mutation, chunk and receipt query passes current
Backend authorization. An enabled feature, Subject, Ticket assignment, path, ID,
or observed validator is not permission. A read-only grant is never upgraded.
The current API has no read-only capability descriptor endpoint: an authorized
WIP inspect describes the operations for the entry kind, while Backend rejects
writes without a write grant. Tool permission configuration also applies to
native Invoke, including fail-closed `ask` where approval cannot be obtained.

Internal SubWorkers (including Reviewer children) have no separate Backend Drive
identity/grant contract today. Spawn suppresses Drive activation **after every
selector resolution**, including `inherit` and explicit builtin Profiles, and
wraps the Workspace client to deny Drive JSON/binary operations. They cannot use
the parent's shared client to inherit Drive read or write. Perform such work in
the authorized parent, or a separately registered Runtime Worker with its own
current Backend grant. Any future internal delegation must introduce an explicit
parent-bounded scope and Backend reauthorization, not remove this fence based on
Profile activation. The wrapper preserves non-Drive Workspace operations and
Reviewer attestation.

## Normal Tools

`DriveRoot {}` discovers the authorized root and returns `{path, metadata}`.
Public entry inputs are **Workspace-bound** references:

```json
{"workspace_id":"workspace-id","node_id":"9007199254740993"}
```

Node IDs remain canonical positive decimal **strings**, including
values above JavaScript's safe-integer range. Renaming/moving preserves IDs and
authenticated latest URLs. Deleting/recreating the same name allocates a new ID.

| Tool | Input/use |
| --- | --- |
| `DriveMetadata` | `entry`: one live entry and its last committed request ID |
| `DriveList` | optional `parent` (default root), `limit` 1–200, opaque `after` |
| `DriveSearch` | `query`, optional `include_text`, `limit` 1–128, opaque `after` |
| `DriveRead` | `entry`, optional `max_bytes` 1–65,536; reports `truncated` |
| `DriveCreateFolder` | `parent`, logical `name` |
| `DriveCreateText` | `parent`, `name`, `content` <=64 KiB, optional `content_type` (Markdown by default) |
| `DriveWrite` | observed `entry`, complete replacement `content` <=64 KiB |
| `DriveEdit` | observed `entry`, `old_string`, `new_string`, optional `replace_all` |
| `DriveRelocate` | observed `entry`, destination `parent`, new/current `name` |
| `DriveDelete` | observed `entry`; folder deletion follows Backend rules |
| `DriveViewImage` | `entry`; bounded committed-request-bound image bytes, not a URL |
| `DriveSaveWorkdir` | explicit `target_workdir` alias, relative `path`, destination `parent`, `name`, optional `content_type` (octet-stream by default) |
| `DriveRequestStatus` | `request_id` from an uncertain mutation |

List/search are paged discovery only, not implicit mutation observations. Omitting
`limit` uses the Backend maximum (200 for list, 128 for search) in both Tools and
native WIP; use the returned `next_after` to continue. Read or
metadata-inspect an entry before replacing, editing, moving or deleting it. The
Tools adapter retains at most 256 observations, scoped to its injected Workspace.
Eviction requires a new explicit observation; it never substitutes another entry.
A successful mutation removes the prior Tools observation. This is a bounded
precondition cache, not a grant cache, second hierarchy DB, or durable history of
all Drive versions. Restored workers observe again before mutating.

`DriveEdit` uses the same pure `fs_operation::text` rules as Workdir and Workspace
config: nonempty `old_string`, different replacement, exactly one nonoverlapping
match unless `replace_all=true`, and bounded resulting text. It fetches a full
<=64 KiB preimage and verifies the **previously observed** committed request ID. It cannot edit
truncated content. The Backend's atomic CAS still decides the first winner.

## Native WIP 0.2.0

Start with `Inspect {"path":"/drive"}`. `/drive` offers root/metadata, list/search,
creation, Workdir save and receipt query. Invoke selects the structured Interface
reference shown by Inspect:

```json
{
  "path":"/drive",
  "interface":{"scope":"/drive","name":"yoi.drive/entry/v1"},
  "operation":"list",
  "arguments":{"limit":20}
}
```

Direct entry paths returned by discovery are:

```text
/drive/<hex-encoded UTF-8 Workspace ID>/<canonical decimal node ID>
```

Use the returned path rather than hand-constructing it. A file exposes metadata,
read, write/edit, relocate/delete and view_image; a folder exposes metadata,
list, create_folder/create_text, save_workdir and relocate/delete where valid.
A native target binds its entry or creation parent: no ID or mutation-precondition argument is
needed. Relocate's `parent` is a WIP typed entry path. Each entry is self-scoped;
Host captures target, scope object, descriptor, validator and handler together.
Client retains and supplies metadata automatically. Do not copy validators or
`scope_ref` through model arguments.

Drive has **no indexable entry children**, regardless of the size of the Drive or
the Client's already-inspected entries. Tree shows the Drive entrance, not a file
inventory. List/search return bounded metadata and paths; only inspect the
selected path. No Worker index/embedding or eager body injection is added.

## Conflicts, unknown outcomes and publication

Mutations use a Host-generated request identity, observed node ID and last
committed request ID. Conflict never causes an adapter to replace that observation with
latest and retry. Observe explicitly, reconsider the edit and submit a deliberate
new operation. Native Client stale handling likewise does not replay dispatch.

Timeout, lost response, oversized/invalid mutation response or invalid completion
may mean **OutcomeUnknown**, including a completion with a mismatched request ID
or invalid entry metadata. The adapter queries the *same* request receipt once;
only a committed receipt with matching IDs and valid metadata returns confirmed
DB success. Otherwise it preserves unknown and the `request_id`. Query
`DriveRequestStatus`; an `uncommitted` snapshot is not
proof that an in-flight request cannot later commit. Do not retry blindly with a
fresh request ID. Backend grants also protect receipt lookup/replay. Safe typed
Denied/NotFound/Conflict/Invalid/Limit/StorageUnavailable/OutcomeUnknown errors are
classified without reflecting provider paths, SQL, transport URLs or secrets.

Saving a Workdir artifact reads through the selected alias's scoped session
`read_bytes`, checks path/offset/size/hash on every bounded chunk and verifies the
assembled SHA256. Source read and destination write are independent. Unknown
aliases, outside-scope paths, traversal/absolute host paths, changed sources,
source read denial, oversized bodies and destination denial cannot publish a
success. Upload transfers raw bytes internally with size/SHA256 metadata; the
model never has to use Bash, a raw host path, or generate base64. Success is the
Backend's **DB-publication commit**, not merely blob transfer completion.

## Images and historical observations

ViewImage reads <=10 MiB PNG/JPEG/GIF/WebP bytes in <=64 KiB Drive chunks at the
metadata’s last committed request ID. Every chunk is authorized and committed-request-bound. An intervening
update expires the read; no mixed-generation image is attached. Media is detected
from bytes using the existing image helper, not the filename or a download URL.

Tools and native Invoke return `ImageAttachment` through `ToolOutput`. Native
attachments accompany—not replace or bypass validation of—the WIP return value.
Engine history, durable Session capture/log replay and normal ToolResult pruning
own those bytes. Later Drive updates/deletion/revocation do not rewrite the text
or image already recorded in a Session; restore reproduces that observation until
normal pruning/correction removes details append-only. This is not full Drive
version history and does not use unrecorded per-request message insertion.

## Validation seams

- `cargo test -p worker --lib feature::builtin::drive`
- `cargo test -p worker --lib native_invoke`
- `cargo test -p worker --lib native_attachments_do_not_bypass`
- `cargo test -p worker-runtime --lib binary`
- `cargo test -p yoi-workspace-server --lib worker_drive_tests`

Server adapter tests use isolated DB + LocalFileSystem + production typed HTTP
router and fresh Runtime proofs/current Worker grants. Worker tests cover
bounded observations, exact CAS inputs, no blind resend, Tools/WIP schemas,
conditional registration, child denial and native image capture/replay. See
`AGENTS.md` for root check, changed-crate tests, format and diff requirements.
