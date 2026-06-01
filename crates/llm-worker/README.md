# llm-worker

## Role

`llm-worker` owns provider-independent model turn orchestration over committed history, tools, callbacks, retries, continuation, pruning, and compaction boundaries.

## Boundaries

Owns:

- Worker history mutation and append contracts
- tool-call loop semantics
- pre-stream retry and stream-started continuation policy
- pruning/compaction coordination from the Worker perspective
- provider-neutral events/callbacks/interceptors

Does not own:

- Pod names, sockets, process lifecycle, or scope delegation (`pod`)
- product CLI shape (`yoi`)
- provider catalog and secret resolution (`provider`, `secrets`)
- durable Pod current state (`pod-store`)

## Design notes

The Worker is where turn lifecycle belongs because it sees history, in-flight usage, partial output, and tool-call state. It should not receive context-only volatile facts; model-affecting inputs must first be appended to history.

## See also

- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
- [`../../docs/design/compaction.md`](../../docs/design/compaction.md)
- [`../../docs/design/provider-model-boundary.md`](../../docs/design/provider-model-boundary.md)
