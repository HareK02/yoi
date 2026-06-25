# pod-store

## Role

`pod-store` is the legacy-named crate that owns current Worker metadata keyed by Worker name.

## Boundaries

Owns:

- persisted Worker metadata files
- current active/pending session pointers
- resolved manifest snapshots for restoration
- parent-visible spawned-child metadata
- restoration labels and diagnostics derived from metadata

Does not own:

- replayable conversation logs (`session-store`)
- live process locks or socket reachability (`pod-registry`, `client`)
- product CLI behavior (`yoi`)
- model turn execution (`llm-engine`)

## Design notes

Worker metadata is intentionally thin. It should answer current-state questions without duplicating transcripts or becoming a second session log.

## See also

- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
