# pod-registry

## Role

`pod-registry` is the legacy-named crate that tracks live Worker process ownership and delegated scope locks at runtime.

## Boundaries

Owns:

- machine-local live Worker registration
- collision detection for running Worker names
- delegated scope lock bookkeeping
- registry cleanup hooks for stopped or unreachable children

Does not own:

- durable Worker metadata (`pod-store`)
- replayable session logs (`session-store`)
- socket protocol definitions (`protocol`)
- project work item state

## Design notes

The registry is a runtime coordination mechanism. It can help decide whether a Worker is live or colliding, but durable visibility/restoration should be backed by Worker metadata when possible.

## See also

- [`../../docs/design/worker-session-state.md`](../../docs/design/worker-session-state.md)
- [`../../docs/design/tool-permissions-scope.md`](../../docs/design/tool-permissions-scope.md)
