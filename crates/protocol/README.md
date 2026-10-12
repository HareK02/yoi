# protocol

`protocol` defines input, control, event, and public history types shared by Workers
and clients. It does not own transport, stored-log replay, or rendering. It separates
live state from committed history so clients can reconnect without depending on storage formats.

## Design entry points

- [`Method`] / [`Event`]: input acceptance versus live updates and history commits
- [`WorkerStateSnapshot`]: controller state versus Runtime catalog state
- [`SessionSnapshot`]: public representation of committed history
- [`subscription`]: ordering of snapshots and subsequent events
- [`invocation`]: validation of typed Feature input
