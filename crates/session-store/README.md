# session-store

## Role

`session-store` owns replayable append-only session logs.

## Boundaries

Owns:

- session identifiers and segment lineage
- JSONL log entries for replayable conversation/runtime history
- restoring Engine/session state from committed records
- schema surfaces that should make drift compile-visible

Does not own:

- current Worker-name metadata (`session-store` worker metadata)
- live process/socket discovery (`worker-allocation`, `client`)
- UI state (`tui`)
- generated memory summaries (`memory`)

## Design notes

A session log records what happened. It is not the current Worker registry and should not be queried as the only source of "what does Worker X mean now?"

Prefer explicit current log variants over broad legacy compatibility when schema changes; hidden compatibility can make future replay bugs silent.

## See also

- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
