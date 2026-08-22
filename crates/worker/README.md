# worker

## Role

`worker` turns an `agen` Engine into a named runtime entity with manifest configuration, scoped tools, session persistence, protocol handling, and Worker metadata integration.

## Boundaries

Owns:

- Worker lifecycle and socket protocol serving
- Engine construction around a resolved Manifest
- session-store and session-store worker metadata coordination
- built-in tool registration under scope/policy
- spawned-child orchestration hooks

Does not own:

- provider-specific wire formats (`provider` / `agen` clients)
- product CLI parsing (`yoi`)
- TUI display authority (`tui`)
- current-state storage schema outside Worker metadata (`session-store` worker metadata)

## Design notes

A Worker is runtime authority, not UI state. It should commit model-visible events through history/session paths and keep current Worker-name state in Worker metadata rather than in transient runtime files.

## See also

- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
