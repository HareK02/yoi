# lint-common

## Role

`lint-common` contains shared linting primitives for structured project records.

## Boundaries

Owns:

- reusable frontmatter/slug/path validation helpers
- common lint diagnostic shapes used by higher-level crates

Does not own:

- memory-specific policy (`memory`)
- workflow-specific policy (`workflow`)
- product CLI command shape (`yoi`)
- work item script behavior (`tickets.sh`)

## Design notes

Keeping common lint mechanics here avoids duplicating low-level validation while leaving each record type's policy in its owning crate.

## See also

- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
- [`../../docs/development/work-items.md`](../../docs/development/work-items.md)
