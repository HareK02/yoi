# agen

`agen` integrates streaming LLM responses and tool execution in an agent engine.
It manages turns, history appends, pause/resume, and retries, while the LLM client
layer handles wire formats and authentication. The caller owns history storage,
worker lifecycle control, and compaction.

## Design entry points

- [`Engine`]: turn execution, interruption, and resumption
- [`History`]: model input paired with caller-owned annotations
- [`state`] and [`prune`]: prefix protection and request-only history projections
- [`llm_client`] and [`timeline`]: wire-format conversion and event processing
- [`interceptor`]: host-owned control decisions
