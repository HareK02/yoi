# session-metrics

## Role

`session-metrics` records usage and memory/session metrics that are useful for diagnostics and maintenance.

## Boundaries

Owns:

- metric record types and persistence helpers
- explicit memory usage/read/reference observations where applicable
- lightweight diagnostic data that should not become model context by itself

Does not own:

- prompt context packing (`llm-engine`)
- generated memory contents (`memory`)
- provider billing semantics (`provider`)
- UI status rendering (`tui`)

## Design notes

Metrics are observations. They may guide compaction, memory effectiveness analysis, or UX, but they are not authoritative conversation history and should not smuggle hidden state into model input.

## See also

- [`../../docs/design/memory-knowledge.md`](../../docs/design/memory-knowledge.md)
- [`../../docs/design/compaction.md`](../../docs/design/compaction.md)
