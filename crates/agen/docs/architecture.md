# agen architecture

`agen` separates orchestration, event projection, and provider transport so applications can replace an LLM client without changing the turn loop or tool model.

```text
┌────────────────────────────────────────────┐
│ Engine                                     │
│ turn loop · interceptors · tool execution  │
│ typed state: Mutable → Locked → Mutable    │
└─────────────────────┬──────────────────────┘
                      │
┌─────────────────────▼──────────────────────┐
│ Timeline                                   │
│ event dispatch · block collectors          │
└─────────────────────┬──────────────────────┘
                      │
┌─────────────────────▼──────────────────────┐
│ LlmClient                                  │
│ transport · provider wire-format schemes   │
└────────────────────────────────────────────┘
```

## Main modules

| Module | Responsibility |
|---|---|
| `engine` | Turn execution, pause/resume, retries, tool integration, and callbacks |
| `state` | Sealed `Mutable` and `Locked` type-state markers |
| `interceptor` | Application-owned control decisions at orchestration boundaries |
| `tool` / `tool_server` | Tool metadata, registration, execution, and bounded output |
| `timeline` | Streaming event dispatch, handlers, and block assembly |
| `llm_client` | Provider-neutral request, response, auth, transport, and scheme contracts |
| `providers` | Optional higher-level provider adapters such as the `codex` feature |
| `prune` / `token_counter` | Cache-aware history reduction and token estimation |
| `usage_record` | Request and token usage accounting |

## Request flow

```text
Engine history
  → provider-neutral Request
    → Scheme::build_request
      → Provider transport
```

## Response flow

```text
streaming response bytes
  → Scheme event parsing
    → unified Event values
      → Timeline handlers and collectors
        → Engine history/tool decisions
```

## Type state and cache protection

`Engine<C, Mutable>` permits configuration and history editing. `Engine::run` or `Engine::lock` commits the current prefix and produces `Engine<C, Locked>`. The locked engine may append turns without mutating the committed prefix. `Engine::unlock` explicitly returns to mutable state when an application accepts losing that cache guarantee.

## Public surface

The 0.2 series exposes the low-level client, timeline, tool, pruning, and usage modules because custom clients and orchestration hosts build directly on them. These APIs are intentionally provider-neutral but remain pre-1.0 and may change in later minor releases.
