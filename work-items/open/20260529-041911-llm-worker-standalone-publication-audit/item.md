---
id: 20260529-041911-llm-worker-standalone-publication-audit
slug: llm-worker-standalone-publication-audit
title: Prepare LLM-Worker for standalone publication
status: open
kind: audit
priority: P2
labels: [llm-worker, docs, api, release]
created_at: 2026-05-29T04:19:11Z
updated_at: 2026-05-29T04:19:11Z
assignee: null
legacy_ticket: null
---

## Background

`llm-worker` is currently developed as part of the Insomnia workspace, but it is intended to be useful as a standalone library for building autonomous LLM-powered systems.

Before publishing or presenting it independently, audit and polish the crate's public surface, documentation, examples, and wording so it can stand on its own without assuming Insomnia-specific context.

This is primarily an audit/preparation ticket. Implementation changes should be limited to documentation polish, small API text/name cleanups, metadata fixes, and clearly safe public-surface adjustments. If the audit finds larger API redesign needs, record them as follow-up tickets rather than mixing a broad refactor into this work.

## Scope

Inspect and update, at minimum:

- `crates/llm-worker/Cargo.toml`
  - description
  - version readiness
  - license/categories/keywords/repository/readme/include/exclude metadata where appropriate
  - dependency choices and feature flags that matter for standalone consumers
- `crates/llm-worker/README.md`
  - standalone crate overview
  - core concepts
  - minimal usage example
  - provider/model configuration assumptions
  - tool/interceptor concepts
  - streaming/retry/continuation limitations
- `crates/llm-worker/docs/*`
  - architecture and requirements accuracy
  - wording that assumes the full Insomnia application
- public Rust API docs
  - crate-level docs (`lib.rs`)
  - public structs/enums/functions/traits likely to appear in rustdoc
  - examples should compile or be marked clearly as illustrative
- examples/tests under `crates/llm-worker`
  - ensure they are understandable to external users
  - avoid leaking local project assumptions or private operational names

## Audit questions

- Can a reader understand what `llm-worker` does without knowing Insomnia Pod/TUI internals?
- Is the public API coherent as a standalone library boundary?
- Are names and docs provider-neutral where possible?
- Are Insomnia-specific terms either absent from public docs or explicitly framed as one possible host application?
- Are error types, interceptors, tool traits, retry/continuation behavior, and history handling documented enough for external use?
- Does crate metadata look publishable?
- Are there obvious public APIs that should be hidden, renamed, or documented before publication?

## Requirements

- Produce a written audit summary in the ticket artifacts directory.
- Apply small documentation/metadata/API wording fixes that are clearly safe.
- Do not perform broad API redesign in this ticket.
- Do not change runtime behavior unless a tiny fix is required to make docs/examples accurate.
- If API redesign is needed, propose follow-up tickets with concrete scope.
- Keep Insomnia workspace behavior unchanged.

## Acceptance criteria

- `crates/llm-worker` has standalone-oriented README / docs wording.
- Crate metadata is reviewed and updated where appropriate.
- Public rustdoc entry points are checked for missing or Insomnia-specific wording.
- An audit artifact records:
  - reviewed files/commands
  - public API concerns
  - docs wording concerns
  - metadata/readiness concerns
  - recommended follow-up tickets
- Any small fixes are committed with the audit.
- `cargo fmt --check`
- `cargo check -p llm-worker`
- Relevant `cargo test -p llm-worker` or focused tests/examples where practical.
- `cargo doc -p llm-worker --no-deps` succeeds or known warnings/issues are recorded.

## Out of scope

- Publishing to crates.io.
- Renaming the crate.
- Extracting the crate into a separate repository.
- Large public API redesign.
- Reworking provider implementations.
- Changing Insomnia Pod/TUI behavior.
