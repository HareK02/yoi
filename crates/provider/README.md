# provider

## Role

`provider` resolves model/provider configuration and constructs provider-specific LLM clients for `llm-worker`.

## Boundaries

Owns:

- builtin and user provider/model catalog resolution
- model reference expansion into concrete model config
- auth reference resolution through supported mechanisms
- provider/scheme capability and context-window metadata
- provider-specific client construction

Does not own:

- Worker turn lifecycle (`llm-worker`)
- secret storage internals (`secrets`)
- Pod lifecycle (`pod`)
- product CLI parsing (`yoi`)

## Design notes

Provider API facts drift. Keep wire-format, auth, catalog, and capability differences here so Worker semantics remain stable.

Codex OAuth is a separate integration from normal provider secret refs because its local file shape and lifecycle differ.

## See also

- [`../../docs/design/provider-model-boundary.md`](../../docs/design/provider-model-boundary.md)
- [`../../docs/design/profiles-manifests-prompts.md`](../../docs/design/profiles-manifests-prompts.md)
