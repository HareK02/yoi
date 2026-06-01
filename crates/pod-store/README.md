# pod-store

## Role

`pod-store` owns current Pod metadata keyed by Pod name.

## Boundaries

Owns:

- persisted Pod metadata files
- current active/pending session pointers
- resolved manifest snapshots for restoration
- parent-visible spawned-child metadata
- restoration labels and diagnostics derived from metadata

Does not own:

- replayable conversation logs (`session-store`)
- live process locks or socket reachability (`pod-registry`, `client`)
- product CLI behavior (`yoi`)
- model turn execution (`llm-worker`)

## Design notes

Pod metadata is intentionally thin. It should answer current-state questions without duplicating transcripts or becoming a second session log.

## See also

- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
