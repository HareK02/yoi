# client

## Role

`client` contains reusable socket-client and runtime-command mechanics for talking to Workers from CLI/TUI code.

## Boundaries

Owns:

- one-shot Worker socket client behavior
- request/reply delivery mechanics
- runtime command construction below the product façade
- shared attach/status probing helpers used by higher layers

Does not own:

- product command names (`yoi`)
- Worker state authority (`worker`, `session-store` worker metadata)
- UI rendering (`tui`)
- Engine turn semantics (`llm-engine`)

## Design notes

The client boundary lets `tui` and `yoi` share Worker communication without making library crates depend on the product binary. Socket clients should drain connect-time snapshot/alert traffic before sending a method or deciding status.

## See also

- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
- [`../../docs/design/overview.md`](../../docs/design/overview.md)
