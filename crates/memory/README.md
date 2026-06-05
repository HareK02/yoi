# memory

## Role

`memory` owns generated memory, Knowledge records, staging/consolidation mechanics, linting, and audit observations.

## Boundaries

Owns:

- memory/Knowledge record parsing and validation
- memory lint subcommand backend behavior
- staging and consolidation file mechanics
- audit log observation writes
- generated memory prompt support where it belongs below the CLI

Does not own:

- authoritative project records (`.yoi/tickets/`, git history)
- normal Pod turn orchestration (`llm-worker`)
- product CLI command shape (`yoi`)
- curated workflow definitions (`workflow`)

## Design notes

Memory is useful context, not authority. It should preserve small durable preferences and rationale without duplicating tickets, docs, or implementation reports.

## See also

- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
- [`../../docs/development/work-items.md`](../../docs/development/work-items.md)
