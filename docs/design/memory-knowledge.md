# Workspace Memory authority

Generated Memory is durable Workspace context owned by the Workspace Server control plane. Runtime Workers query and mutate it through typed Workspace API operations; they do not read or write repository-local paths.

The active Memory document and staging/audit lifecycle are stored by the Server-owned backend. Worker-side `memory` code contains only shared schema, extraction, audit, and transport types needed to construct those operations.

Repository or ancestor `.yoi/memory`, `.yoi/knowledge`, and malformed marker trees are ignored. There is no cwd/ancestor discovery, dual read, startup import, or automatic migration. The removed `yoi memory lint` command is intentionally not a compatibility surface; invoking `yoi memory ...` is an unknown command.

Memory remains supporting context rather than implementation authority. Use Tickets, Objectives, repository files, Git history, and append-only session records for exact current facts.

## Upgrade guidance

Old repository-local Memory data is not imported automatically. Before upgrading from a version that used `.yoi/memory`, explicitly export any data that must be retained with that old version and import it into the Workspace-backed product surface available to the deployment. Stale files may be archived or deleted after verification.

## Preserved filesystem boundaries

This removal does not affect repository checkouts, Git metadata, Runtime-owned Workdirs/session logs/artifacts, Server data directories, or XDG client/server configuration. It removes only ambient repository-local Memory authority.
