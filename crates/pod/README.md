# pod

## Role

`pod` turns an `llm-worker` Worker into a named runtime entity with manifest configuration, scoped tools, session persistence, protocol handling, and Pod metadata integration.

## Boundaries

Owns:

- Pod lifecycle and socket protocol serving
- Worker construction around a resolved Manifest
- session-store and pod-store coordination
- built-in tool registration under scope/policy
- spawned-child orchestration hooks

Does not own:

- provider-specific wire formats (`provider` / `llm-worker` clients)
- product CLI parsing (`yoi`)
- TUI display authority (`tui`)
- current-state storage schema outside Pod metadata (`pod-store`)

## Design notes

A Pod is runtime authority, not UI state. It should commit model-visible events through history/session paths and keep current Pod-name state in Pod metadata rather than in transient runtime files.

## See also

- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
