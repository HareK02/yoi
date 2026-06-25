# protocol

## Role

`protocol` defines the JSONL message boundary between Pod clients and Pod servers.

## Boundaries

Owns:

- transport-neutral method/event/result types
- request/reply and broadcast event shapes
- protocol error categories shared by clients and servers

Does not own:

- Unix socket implementation details (`client`, `pod`)
- TUI rendering (`tui`)
- Engine history semantics (`llm-engine`)
- durable storage (`session-store`, `pod-store`)

## Design notes

The exact enum variants are code authority. The README should describe the boundary, not duplicate every message shape.

Protocol events can inform UI and orchestration, but durable state changes still need to flow through Pod/session/metadata records.

## See also

- [`../../docs/design/pod-session-state.md`](../../docs/design/pod-session-state.md)
- [`../../docs/design/context-history.md`](../../docs/design/context-history.md)
