# tui

`tui` is a terminal client for a single Worker's Console and the Workspace Dashboard. It handles input editing, display state, and connection-specific screen transitions. It does not own Session persistence or Worker lifecycle decisions.

## Implementation entry points

- [`launch`] / [`LaunchMode`] — typed launch requests and terminal restoration.
- `src/app.rs`: display state built from protocol snapshots and events.
- `src/input.rs` / `src/composer_history.rs`: typed input and local editing history.
- `src/console/`: connection, input-event, and attachment-staging management.
