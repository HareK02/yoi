# Repository, Workdir and attachment WIP projection

## Objects and operations

The Host owns these root namespaces, using the same `WipMountRegistry`
namespace/resolver/contribution contracts as Tickets, Objectives and Merge Requests:

```text
/
├── repositories                  list
│   └── <encoded repository key>  read
├── workdirs                      list, create
│   └── <encoded Workdir ID>      read, attach, delete (when supported)
└── workdir-attachments            list
    └── <connection lifetime ID>   read, detach
```

Operations are Interface declarations, **not child files**. There is no
`/features/manage-workdir`, no writable Repository catalog and no Repository
content tree here. `/checkouts` content projection is a separate provider (T-699),
as is Workspace configuration (T-695). Management Objects do not grant access to
unattached contents. A future content provider should return its entrance from
an authorized, live attachment; it must not infer content/command capability
from catalog or lifecycle access.

`path` in inventory results is an absolute WIP Object path. It is a reference,
not an OS path, a capability grant, or an instruction to reinterpret another
string argument as a route. Source Repository references use `repository_path`;
attachment references use `workdir_path`. Attach results include
`attachments_path` to discover the current connection and its lifetime ID.

### Identity and route encoding

Repository identity is the formal Backend `repository_key`, never its display
name or a guessed `main`. Workdir identity is `working_directory_id`, never a
Worker-local alias. A segment containing only ASCII letters, digits, `-`, `_`
is unchanged. All other UTF-8 identities are encoded as `~` followed by lowercase
hexadecimal UTF-8 bytes. The `~` marker is itself always encoded. Only the
canonical encoding is accepted; no alternate spellings, recursive URL decoding,
case folding, slash traversal or namespace fallback are accepted. For example,
`repo/name` is `/repositories/~7265706f2f6e616d65`. This is injective and accepts
existing valid keys, including Unicode and slash-containing identities.

Attachment identity is a fresh opaque Backend connection ID for each lifetime,
including detach/reattach to the **same** alias and Workdir. Read exposes alias,
Workdir ID and effective attachment capabilities as attributes. Workdir existence
and Worker connection are separate Objects. Detaching a connection does not
remove the Workdir or delete a logical settings space.

## Feature and authority separation

`feature.workdir_catalog.enabled` independently supplies read-only Repository,
Workdir and attachment reference operations. It defaults enabled so a WIP Worker
with Workspace authority and appropriate permissions can find registered keys
without Ticket or conversation hints. It does not add ordinary Tools or require
a filesystem/Bash location. Standalone Workers without Workspace authority have
no Workspace catalog mounted.

`feature.manage_workdir.enabled` supplies management operation implementations
on the **same Objects** using `contribute_operations` and
`contribute_dynamic_operations`. It remains default disabled. It neither owns
another resolver nor confers permission. The object provider is unique; duplicate
namespace, route, operation name or inconsistent Interface contributions fail at
registration through the common registry.

Native permission identities are:

| Operation | Manifest permission identity | Bound identity in permission input |
| --- | --- | --- |
| repositories.list | RepositoryList | list arguments |
| repository.read | RepositoryRead | repository_key |
| workdirs.list | WorkdirList | list arguments |
| workdir.read | WorkdirRead | working_directory_id |
| workdirs.create | WorkdirCreate | original create arguments |
| workdir.attach | WorkdirAttach | working_directory_id + alias |
| workdir.delete | WorkdirDelete | working_directory_id + reason |
| attachments.list | WorkdirAttachmentList | list arguments |
| attachment.read | WorkdirAttachmentRead | connection_id |
| attachment.detach | WorkdirDetach | current bound alias |

Read permissions gate direct item resolution as well as list item visibility.
The manifest's existing default policy is retained; `ask` is denied fail-closed.
An argument-dependent policy may advertise an operation that could be allowed,
but every call checks its exact reconstructed input before execution. Mutation
arguments cannot override path-bound Workdir or connection identity. Having
RepositoryRead is not WorkdirCreate; inventory/management permissions are not
file access, delegation or command execution grants. Backend caller/Workspace
identity, source proof, attachment capability ceilings and lifecycle policy are
checked by the existing paths on every request.

## Contextual Interfaces and stale observations

Providers opt into common Host contextual Interfaces. Operation publication is
the intersection of registered implementation, current target support and current
subject policy. The collection reference feature can remain available while the
management feature is disabled. External Workdirs without cleanup authority do
not expose delete. Only currently supported lifecycle operations are advertised;
no delete/command is added to logical attachments solely because they appear as
workspaces.

A contextual Interface reference is opaque and path-qualified as
`<base>/@/<hex UTF-8 Object path>`. Clients must use the returned reference, not
construct it. Fetch and call freshly resolve the same Object and filtered
Interface; base references cannot fetch an unfiltered descriptor. Descriptor and
Object validators invalidate old observations after target/connection/permission
changes. Validators are managed by the WIP Client, not manually by the LLM.
Cached Interface possession and known direct paths never bypass current checks.
Historic session text remains append-only and is not rewritten or replayed.

## Backend lifecycle and safe output

Catalogs are request-time projections of existing Backend authority, not another
store or synchronization job. WIP list pages default to 50 and accept 1–100
items. Repository pages sort the existing complete Repository catalog by stable
key. Workdir pages use the signed caller-bound Backend
`workers/self/workdir-catalog` operation, with SQL keyset paging over the existing
Workdir registry in stable ID order. This is not the legacy bounded
`working-directories` browser/Tool snapshot: Workdirs older than its 200-record
cap remain reachable. `next_cursor` advances over scanned rows even when the
current read policy or unsupported legacy source makes a page sparse or empty;
`empty` describes that page, and `has_more` remains authoritative for progress.
Attachment pages use the Backend's SQL-bounded, alias-ordered limit/offset query
and opaque `offset:<n>` cursors. Both Workdir and attachment responses supply an
opaque complete-set `revision` derived from existing authoritative records in
the same database snapshot as the page, not by hashing the first page. The Host
uses this complete-set revision for the collection Object validator. A lifetime
or capability change beyond row 50 therefore invalidates old collection
observations without adding a ledger or revision store. A cursor is not
a snapshot or authority grant: restart listing after concurrent collection
changes. Filtering by exact `connection_id` is applied in the caller-scoped
Backend ledger before paging, so direct item lookup also works beyond the first
page. Unknown/expired IDs yield no item, never a current alias substitute.
Repository projection uses an explicit field
allowlist: formal key, kind/provider, any existing display metadata and explicit
nullable default selector. Source URI/path, credentials, remotes, Runtime
connection information and internal fingerprints are not model-visible.
Workdir projection includes stable ID, optional display name, public source kind
and Repository key, existing selector/ref/tree, status, cleanliness and occupancy
when available. Cleanup handles and External grant internals are not forwarded.
No missing attributes or implicit selectors are fabricated. Backend errors are
sanitized rather than forwarding raw diagnostic bodies with possible host paths.

Create and attach reuse the ordinary Workdir backend, request identity,
Worker-session router, mutation lock and Internal SubWorker lifecycle hooks.
Creation does **not** attach. Attach installs the Backend-attenuated session
capabilities, never a fabricated `ALL` capability for External Workdirs. Detach
uses the same begin/finish/cancel-detach path and command/operation exclusion as
Tools; the Backend checks expected connection identity under its existing
current-Worker lifecycle lock before closing sessions, then conditionally
updates that exact row in the existing connection ledger. Alias reuse cannot
retarget an old connection operation. Compensation retains the original
connection identity. Existing removal dispositions `removed`, `retained`, and
`attention_required`, plus retryability, remain intact.

Stale identity/validator rejection is distinct from unknown operation outcome.
Transport failure, invalid post-dispatch response or uncertain lifecycle failure
is **not** proof that no effect occurred; it must not be automatically retried.
Inspect current state or request operator guidance. No new retry or attachment
ledger is introduced by WIP.

When native management is mounted, `WorkdirList/Create/Attach/Detach/Delete` are
claimed as represented native capabilities, so `/tools/Workdir*` is not also
published. Ordinary Tools mode keeps its names and inputs. Existing native
Ticket/Objective/Merge Request routes retain their T-698 contracts.

## First-Workspace example

1. `Tree(path="/", depth=1)` finds `repositories`, `workdirs` and
   `workdir-attachments` when permitted.
2. Inspect the Repository collection path and Invoke `list` with a
   bounded limit. An empty registered/visible catalog explicitly returns an empty
   list; do not invent a key. Follow `next_cursor` when supplied.
3. Inspect the returned item `path` directly and Invoke `read`.
   `default_selector: null` means no configured default, not `main`.
4. Inspect the Workdir collection and `create` using the discovered formal key,
   with optional selector, display name and Runtime under the existing contract.
5. Follow created `item.path`, Inspect directly and Invoke `attach` with a chosen
   Worker-local alias. This is separate from creation.
6. Follow `attachments_path`, `list`, then discover/read the returned connection
   path to confirm alias, Workdir and effective capabilities.
7. Call that connection's `detach` with no identity override. The observed
   lifetime ID guards alias reuse; the Workdir remains in its inventory.

Each step uses current WIP observations and no Ticket-derived key, host path,
Runtime endpoint or manual validator. Explicitly refresh/reset discovery after
external changes and before reconstructing a rejected stale call.
