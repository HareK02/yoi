# workflow

## Role

`workflow` owns project-authored workflow record parsing, validation, and invocation metadata.

## Boundaries

Owns:

- workflow file schema/linting
- workflow discovery from configured workflow locations
- typed workflow metadata used by runtime/tooling layers

Does not own:

- generated memory records (`memory`)
- Ticket file lifecycle (`crates/ticket`, `.yoi/tickets/`)
- Pod orchestration decisions (`pod`, workflows executed by agents)
- product CLI command shape (`yoi`)

## Design notes

Workflows are curated project assets. They should not be mixed with generated memory, and invoking a workflow should not bypass work item authority or scope policy.

## See also

- [`../../docs/development/workflows.md`](../../docs/development/workflows.md)
- [`../../docs/design/profiles-manifests-prompts.md`](../../docs/design/profiles-manifests-prompts.md)
