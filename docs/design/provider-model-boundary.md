# Provider and model boundary

The Worker should be provider-independent. Provider-specific wire formats, auth mechanisms, model catalogs, reasoning knobs, and terminal error shapes belong below or beside it, not inside ordinary turn orchestration.

## Worker responsibility

`llm-worker` owns turn lifecycle:

- committed history append
- tool loops
- retry before stream start
- continuation after safe partial output
- pruning/compaction decisions
- provider-neutral callbacks and events

It should not know where a Codex OAuth token lives, how a provider encodes reasoning effort, or which environment variables a provider once supported.

## Provider responsibility

Provider/model layers own:

- resolving model aliases and builtin model metadata
- provider-specific request/response conversion
- auth integration
- context/window metadata
- terminal error classification
- trace points around HTTP/auth/body/SSE lifecycle

External API facts change quickly. Repository docs should record Yoi's boundary decisions, not snapshots of vendor documentation.

## Secrets

Local credentials use explicit secret references. Secret values must not appear in diagnostics, Debug output, CLI/TUI output, work items, docs, session logs, model context, or persisted plaintext store files.

Codex OAuth remains a separate integration because its existing `auth.json` / `CODEX_HOME` shape is not the same as normal provider secret refs.

## Error and trace semantics

Provider terminal errors that are displayed live, such as context-length failures, must persist as errored runs rather than successful empty turns.

Event trace sidecars are optional parsed lifecycle traces. They are not complete raw SSE logs and should not become a hidden source of model context.

## Why this boundary exists

Provider APIs drift. If provider details leak into Worker logic, every new model behavior risks changing core orchestration. Keeping the boundary explicit lets Yoi adapt provider integrations while preserving stable turn semantics.
