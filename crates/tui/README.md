# tui

`tui` is a terminal client for a single Worker's Console and the Workspace Dashboard. It handles input editing, display state, and connection-specific screen transitions. It does not own Session persistence or Worker lifecycle decisions.

## Implementation entry points

- [`launch`] — Console and Dashboard execution with terminal restoration.
- [`LaunchOptions`] / [`LaunchMode`] — typed connection and launch requests.
