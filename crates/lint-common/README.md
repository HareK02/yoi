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
- Ticket backend policy (`ticket`)
- product CLI command shape (`yoi`)

## Design notes

Keeping common lint mechanics here avoids duplicating low-level validation while leaving each record type's policy in its owning crate.

## See also

- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
- [`../../docs/development/work-items.md`](../../docs/development/work-items.md)
