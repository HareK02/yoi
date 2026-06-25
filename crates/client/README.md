# client

## Role

`client` contains reusable socket-client and runtime-command mechanics for talking to Pods from CLI/TUI code.

## Boundaries

Owns:

- one-shot Pod socket client behavior
- request/reply delivery mechanics
- runtime command construction below the product façade
- shared attach/status probing helpers used by higher layers

Does not own:

- product command names (`yoi`)
- Pod state authority (`pod`, `pod-store`, `session-store`)
- UI rendering (`tui`)
- Engine turn semantics (`llm-engine`)

## Design notes

The client boundary lets `tui` and `yoi` share Pod communication without making library crates depend on the product binary. Socket clients should drain connect-time snapshot/alert traffic before sending a method or deciding status.

## See also

- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
- [`../../docs/design/overview.md`](../../docs/design/overview.md)
