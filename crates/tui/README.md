# tui

## Role

`tui` implements terminal UI clients for interacting with one or more Pods.

## Boundaries

Owns:

- terminal rendering and input handling
- local composer state and UI affordances
- single-Pod attach/restore screens
- multi-Pod dashboard presentation

Does not own:

- durable transcript authority (`session-store`)
- Pod current state (`pod-store`)
- Pod lifecycle policy (`pod`)
- product CLI ownership (`yoi`)

## Design notes

The TUI should display committed events and Pod snapshots rather than inventing durable state. Local input history and optimistic UI affordances are editing conveniences; they must not become hidden model context.

## See also

- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
