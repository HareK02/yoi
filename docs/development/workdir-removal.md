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
hash surviving checkout content. The saved removal witness is not a current
cleanliness assertion; only an actual removal retry runs the full content checks.

## Runtime safety boundary

The Runtime keeps removal content/identity evidence outside the tree being
removed. This is a factual witness for partial deletion, not a new recovery
state or cleanup stage. Surviving files are rechecked against it; an intact,
currently clean checkout can renew obsolete evidence without discarding newly
ignored/untracked content. Do not edit this witness manually.

Mount-safe removal currently requires Linux `openat2` and `/proc` inspection.
Unavailable inspection fails closed with `mount_check_unavailable`; there is no
fallback to recursive deletion across an unchecked filesystem boundary.
Runtime HTTP list, detail and removal requests run synchronous provider work on
blocking workers rather than the HTTP executor. Removal reserves only the target
Workdir under the Runtime state mutex, then releases that mutex before content
inspection and unlink. Unrelated Worker list/detail, session observation and
subscription snapshots remain available while removal runs. Conflicting use,
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
