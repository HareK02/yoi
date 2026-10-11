# Workdir removal and ordinary retry

Use the Runtime **Workdirs** page or the normal typed Workdir removal tool to
remove an unused repository Workdir. Deletion is authorized by the Backend's
existing durable removal operation; it is not a registry-only hide operation.

A failed attempt does not require a new recovery state or a manual edit of
`materialization.json`. Retry the same removal request after resolving the
reported cause. Each attempt rechecks current attachments, attachment
reservations, retention holds, materialization identity and current file changes.
A previous clean observation is not permission to discard new changes. A dirty
Workdir must be preserved or its changes resolved by its owner before deletion.
When the same repository checkout is still usable, an authorized Worker may
reattach it through normal `WorkdirAttach` to preserve or resolve changes, then
detach and retry removal. An executing removal fences new attachments; a failed
attempt does not permanently prevent reconnection. Missing or replaced checkout
identity is not made usable by this permission.

## Interpreting a retained Workdir

- **Mount remains:** Git status being clean does not mean mounted resources are
  released. Confirm the mount's owner, users and data destination. Have that
  owner safely release the resource, then retry normal deletion. Yoi does not
  recursively delete mounted data or implicitly unmount external/unknown
  resources. Do not use force-clean, recursive chmod or raw recursive deletion
  to override this boundary.
- **Permission denied / resource busy / storage unavailable:** have the owning
  operator resolve access, release the resource or restore storage. Retry the
  same normal action; registry retention is intentional until cleanup is proven.
- **Changes present / changes cannot be verified:** preserve current files and
  restore repository inspection where possible. Do not treat a previous cleanup
  failure or missing checkout metadata as proof that all remaining files are
  expendable.
- **Identity or ownership unavailable:** verify the Workdir target with its
  owner. Do not rewrite registry or materialization identity to bypass checks.
- **Runtime unavailable / outcome unknown:** reconnect the Runtime and retry.
  Absence of a provider response is not authoritative not-found evidence.
- **Attachment, reservation or hold:** release the current owning authority
  through its normal API before retrying. A stale failure does not freeze these
  conditions; newly attached Workers remain protected.

The Web page refreshes authoritative inventory after each attempt. Successful
removal commits registry deletion only after Runtime confirms physical deletion
or authoritative not-found. An unknown result leaves the registry in place so
retry can reconcile the provider's current state.

A retained partial-removal Workdir is listed as `cleanup_pending` with cleanliness
`unknown`. List/detail observation checks its saved identity but does not read or
hash surviving checkout content. Saved removal authority is not a current
cleanliness assertion; an actual removal retry rechecks current safety conditions
and the metadata of surviving entries.

## Runtime safety boundary

The Runtime keeps removal identity and metadata evidence outside the tree being
removed, in `.cleanup-authority`. This is factual authority for partial deletion,
not a new recovery state or cleanup stage. Do not edit it manually.

### Metadata-only removal authority

Initial cleanup, ordinary retry, partial-removal recovery and each pre-unlink
check use descriptor-relative entry metadata, without following symlinks or
crossing mount boundaries. Cleanup does not read regular-file bodies or compute
content hashes, and does not create, refresh or compare durable content digests.
The inventory records:

- entry identity (device/inode) and filesystem type;
- executable bits for regular files, plus size and modification time with
  nanosecond precision for regular files and symlinks;
- identity and type only for directories. Directory size, permissions and
  timestamps are not a change witness: cleanup's own child unlinks change
  directory timestamps.

This is metadata change protection, not a byte-equality guarantee. Deliberate
same-size writes that restore the recorded timestamp are not promised to be
detected. Symlink targets are not traversed or read for a content digest.
Necessary entry enumeration, metadata inspection and unlink remain; their work
is not proportional to the total regular-file body size. Normal Git inspection
may still read repository content, and the Runtime still reads its small managed
metadata records. Neither is a cleanup-specific full-content scan.

### Clean checkout and ordinary retry

Initial admission and metadata renewal from a usable checkout still require the
normal Git dirty check, including non-ignored untracked files. Dirty content is
not authorized for removal. Ignored entries already present at the initial clean
attempt are included in that attempt's metadata authority; Git ignoring a path
alone is not permission to discard later changes to it.

On retry with an intact, usable Git checkout, new or metadata-changed ignored
entries and entries outside the tracked checkout/Git set are protected with
`changes_present`. A currently clean checkout may renew tracked-file and Git-entry
metadata only after the same Workdir root, checkout and Git identities and
materialization ownership are confirmed. Clean Git does not renew authority for
changed ignored/outside entries. A replaced Workdir root, checkout or Git directory,
or substituted ownership, is rejected with `ownership_unknown`.

With the new metadata format, surviving entries that match a subset of the
persisted metadata authority can be removed after partial deletion even when Git
is no longer usable or present. Any surviving Runtime materialization record must
still be validated as the same materialization. Missing entries do not invalidate
that authority, and directory timestamp changes from earlier unlinks do not block
retry. If new entries or metadata changes break the subset match and usable Git
is unavailable, the partial cleanup returns `changes_unknown`, not
`changes_present`: it cannot establish current change safety or renew the
metadata authority. Unknown partial residue without metadata authority likewise
returns `changes_unknown`; a prior failed attempt is not proof of cleanliness.
Validated metadata-only residue (the Runtime materialization record and, at most,
an empty checkout directory) may finish without Git.

### Existing digest-based authority

Old `.cleanup-authority` entries may contain `digest` fields. Those digests are
ignored/discarded, not recomputed or compared, and do not authorize residual file
content under the new contract. Migration can establish new metadata authority
only with a usable current clean Git checkout, the same root, checkout and Git
identities, and validated materialization ownership. Even then, surviving ignored
entries or other entries outside the tracked checkout/Git set block migration
with `changes_unknown`: legacy entries have no size/mtime metadata against which
to verify these non-tracked survivors. Clean Git alone cannot authorize them.

An incomplete legacy cleanup cannot use the new-format subset rule. Residual
files without usable clean Git return `changes_unknown`; they are not adopted,
and digest regeneration is not required. Metadata-only residue may finish only
with validated saved ownership, confirmed remaining identities, and validation
of any surviving Runtime materialization record. An empty same-identity root can
finish even when its record was already removed. Replaced root/checkout/Git identities or substituted ownership remain
`ownership_unknown`. Ordinary retry and Runtime restart use these same rules;
manual metadata edits and a new recovery lifecycle are not required.

Mount-safe removal currently requires Linux `openat2` and `/proc` inspection.
Unavailable inspection fails closed with `mount_check_unavailable`; there is no
fallback to recursive deletion across an unchecked filesystem boundary.
Runtime HTTP list, detail and removal requests run synchronous provider work on
blocking workers rather than the HTTP executor. Removal reserves only the target
Workdir under the Runtime state mutex, then releases that mutex before metadata
and Git inspection and unlink. Unrelated Worker list/detail, session observation
and subscription snapshots remain available while removal runs. Conflicting use,
attachment, creation or removal of the same Workdir is rejected promptly rather
than waiting under the global mutex.

Provider leases protect in-flight creation/access and live session resources,
including their command tasks. Cloned sessions retain protection until safe close
or final resource release; passive bindings alone do not prevent removal. Removed
bindings cannot activate against a recreated Workdir with the same ID. Creation
owns a fresh root exclusively so failed creation cannot roll back another
creator's checkout or adopt an existing partial-removal directory. Reservations
are transient and released on provider success, error or unwind; they do not
replace the durable cleanup witness or attachment authority.

## Diagnostics

Public responses expose a bounded cause category and safe guidance, not arbitrary
provider errors or host paths. For an OS failure, correlate Backend removal logs
(`operation_id`, `workdir_id`, Runtime) with Runtime cleanup logs for that Workdir
and cleanup action. Runtime logs retain the concrete OS error/errno for the
owning operator. These private diagnostics must not be copied unredacted into
public responses or Tickets.

This procedure is not authorization to manipulate a live mount or another
Worker's storage. If ownership or required operator authority is unclear, stop
and request that specific owner/operator's decision.
