# memory

## Role

`memory` owns the shared types used by Workspace-backed Memory operations:

- Memory API request/response DTOs
- extraction candidate and evidence schemas
- audit event schemas
- pure Memory document frontmatter schemas and parsing helpers

## Boundaries

The crate does **not** own persistence. Workspace Memory records, staging candidates, and audit observations are stored by the Workspace Server control plane and accessed by Workers through typed API operations.

The crate intentionally contains no repository-local layout, cwd/ancestor discovery, resident-file reader, staging writer, audit/usage log writer, generic filesystem scope helper, or local backend executor. The product CLI does not expose `yoi memory lint`.

Memory is useful context, not implementation authority. Tickets, Objectives, repository files, Git history, and append-only session records remain the exact evidence surfaces for implementation state.

## See also

- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
- [`../../docs/development/work-items.md`](../../docs/development/work-items.md)
