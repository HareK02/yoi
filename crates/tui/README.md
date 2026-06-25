# tui

## Role

`tui` implements terminal UI clients for the single-Worker Console and workspace Dashboard surfaces.

## Boundaries

Owns:

- terminal rendering and input handling
- local composer state and UI affordances
- single-Worker Console attach/restore/chat screens
- workspace Dashboard presentation and role-action UI

Does not own:

- durable transcript authority (`session-store`)
- Worker current state (`pod-store`)
- Worker lifecycle policy (`worker`)
- product CLI ownership (`yoi`)

## Design notes

The TUI should display committed events and Worker snapshots rather than inventing durable state. Local input history and optimistic UI affordances are editing conveniences; they must not become hidden model context.

## See also

- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
