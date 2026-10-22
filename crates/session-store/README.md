# session-store

`session-store` stores replayable Session history and current Worker restoration data.
Session history uses append-only JSONL; current Segment references and child/peer
registrations live separately in Worker metadata. It does not own process liveness
checks or Runtime catalog lifecycle management.

## Design entry points

- [`segment_log`] / [`segment`]: history replay and fork/compaction lineage
- [`worker_session_store`]: storage boundary for a Session bound to a Worker
- [`worker_metadata`]: current Worker state and active Segment switching
- [`system_item`]: records of system input presented to the model
- [`capture`]: Worker-independent immutable public values and bounded body transport;
  authorization, source I/O, and delivery lifecycle stay with the Host
- [`public_snapshot`] / [`public_index`]: public projections of stored history
- [`event_trace`]: diagnostic records independent of history replay
