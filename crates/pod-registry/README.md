# pod-registry

## Role

`pod-registry` tracks live Pod process ownership and delegated scope locks at runtime.

## Boundaries

Owns:

- machine-local live Pod registration
- collision detection for running Pod names
- delegated scope lock bookkeeping
- registry cleanup hooks for stopped or unreachable children

Does not own:

- durable Pod metadata (`pod-store`)
- replayable session logs (`session-store`)
- socket protocol definitions (`protocol`)
- project work item state

## Design notes

The registry is a runtime coordination mechanism. It can help decide whether a Pod is live or colliding, but durable visibility/restoration should be backed by Pod metadata when possible.

## See also

- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
