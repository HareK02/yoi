# Design overview

Yoi is organized around one rule: durable authority must live in explicit records, while runtime conveniences remain reconstructable hints.

That rule shapes the crate split. The runtime can restart, attach, compact, or delegate work without relying on hidden controller state, and developer tooling can inspect the records that explain why an agent acted.

## Core layers

- `yoi` owns the product CLI and top-level command shape. It is the façade that wires profile selection, memory linting, and normal TUI launch.
- `pod` turns a `Engine` into a named runtime entity with scope, session persistence, protocol handling, tools, and Pod metadata integration.
- `llm-engine` owns model-facing turns: history append, retries, continuation, pruning/compaction mechanics, tool loops, and provider-independent callbacks.
- `session-store` owns replayable append-only conversation/session logs.
- `pod-store` owns current Pod metadata keyed by Pod name.
- `protocol` defines the socket message boundary between clients and Pods.
- `client` contains reusable one-shot socket/runtime-command mechanics so lower crates do not depend on the product CLI.
- `manifest` resolves Profiles, Manifests, model/provider references, scopes, prompts, and tool permission policy into a runtime contract.
- `tools` implements built-in tools with bounded output and policy-aware execution.
- `memory` owns generated memory, Knowledge records, linting, staging, and audit observations.
- `workspace-server` is the local Workspace control-plane seam. It can project Tickets, Workers, lifecycle, usage, and orchestration events, but browser/API operations must stay on opaque backend identities instead of raw local paths, sockets, Pod names, or session files.
- `tui` is a UI over Pod authority; it should not invent durable state.

## Why these boundaries exist

The Engine should not know process identity, Pod names, live sockets, spawned children, or UI state. It should know how to run an LLM turn over committed history and tools.

The Pod should not make provider-specific wire decisions. It coordinates runtime identity, persistence, scope, and protocol delivery around a Engine.

The TUI should not be an alternate source of truth. It may queue local input, show optimistic affordances, and render snapshots, but durable state comes from Pod/session records.

The CLI should own product command shape. Other crates should expose library APIs and typed runtime commands rather than re-parsing product arguments.

## Documentation boundary

Design docs explain the constraints that code must preserve. They should not duplicate every public type, exact schema field, or command output; those are owned by code, tests, and work items.
