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
