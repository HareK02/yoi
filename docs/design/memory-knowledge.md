# Legacy Workspace Memory authority

The single-document Workspace Memory product is a **legacy/deprecated** compatibility surface. Its document, staging candidates, and staging resolutions are durable Workspace context owned by the Workspace Server control-plane database. It is separate from subject-scoped subjektiv Memory, whose persistence authority is the Workspace's Server-managed `subjektiv` Feature database.

Legacy Runtime Workers query and mutate the old document through the established typed `/api/w/{workspace_id}/memory...` operations and `MemoryQuery`, `MemoryReadDocument`, `MemoryUpdateDocument`, and legacy `MemoryStaging*` tools. While those surfaces remain available, their existing inputs keep their existing Workspace-document meaning. They must not be silently redirected to subjektiv, assigned an inferred subject, or reused with a new interpretation.

Worker-side `memory` code owns shared schemas and transport types, not persistence. Legacy `AppendAudit` is acknowledgement-only and has no dedicated durable Memory-audit rows. General Session logs and diagnostics remain independently durable under their own retention policies.

Repository or ancestor `.yoi/memory`, `.yoi/knowledge`, and malformed marker trees are ignored. There is no cwd/ancestor discovery, dual read, startup import, or automatic migration. The removed `yoi memory lint` command is intentionally not a compatibility surface; invoking `yoi memory ...` is an unknown command.

Memory remains supporting context rather than implementation authority. Use Tickets, Objectives, repository files, Git history, and append-only Session records for exact current facts.

## subjektiv cutover and reset

Enabling `feature.subjektiv` does not migrate or erase legacy rows. The only supported destructive boundary is the explicit, authorized legacy reset API described by the [subjektiv product cutover runbook](../development/subjektiv-product-cutover.md). It atomically deletes the old document, staging candidates, and staging resolutions while retaining Workspace Memory settings, Sessions and diagnostics, Tickets, Knowledge, Skills, the subjektiv Feature database, and unrelated Workspace data. The reset is operator-triggered and idempotent; it is never a startup migration or Profile side effect.

## Upgrade guidance

Old repository-local Memory data is not imported automatically. Before upgrading from a version that used `.yoi/memory`, explicitly export any data that must be retained with that old version and import it into the intended Workspace-backed product surface. Do not assume the legacy control-plane reset imports repository-local or legacy Workspace data into subjektiv. Stale files may be archived or deleted only after verification.

## Preserved filesystem boundaries

Repository-local Memory removal and the control-plane legacy reset do not affect repository checkouts, Git metadata, Runtime-owned Workdirs, Session logs or artifacts, Server data directories, XDG client/server configuration, or the subjektiv Feature database. They remove only their explicitly named legacy authority.
