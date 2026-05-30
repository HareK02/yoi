---
id: 20260530-054927-refresh-builtin-model-catalog
slug: refresh-builtin-model-catalog
title: Refresh builtin model catalog to current provider recommendations
status: closed
kind: task
priority: P2
labels: [models, providers, catalog, research]
created_at: 2026-05-30T05:49:27Z
updated_at: 2026-05-30T23:17:46Z
assignee: null
legacy_ticket: null
---

## Background

`resources/models/builtin.toml` contains the built-in model catalog used by the Profile/Manifest resolver. It currently includes a small set of Anthropic, Codex/OpenAI OAuth, Ollama, and OpenRouter entries. Some entries may be stale, provider-specific recommendations may have changed, and context-window/effective-window metadata should be checked against current provider documentation before public release.

The goal is to research the current provider-recommended model set for the providers already represented in the catalog and update the built-in catalog accordingly. This should not become a broad provider integration redesign.

Current catalog files:

- `resources/models/builtin.toml`
- `resources/providers/builtin.toml`
- `resources/profiles/default.lua` references `codex-oauth/gpt-5.5`

## Requirements

- Research current provider-recommended model IDs and metadata for providers currently represented in the built-in catalog:
  - Anthropic direct provider;
  - OpenAI/Codex OAuth provider;
  - OpenRouter provider;
  - Ollama local placeholder entries, if applicable.
- Use authoritative provider documentation or model-list endpoints where practical. Record sources in the ticket thread or an artifact.
- Update `resources/models/builtin.toml` to replace stale models with current recommended models.
- Preserve or intentionally update default profile model choice in `resources/profiles/default.lua`; if changing it, explain why.
- Verify context window and effective context window metadata, especially for models used by default profiles and compaction heuristics.
- Keep provider definitions in `resources/providers/builtin.toml` unchanged unless model catalog research proves a provider entry itself is stale or wrong.
- Avoid adding speculative models that are not generally available or not supported by the current provider client implementation.
- Keep local/Ollama entries generic unless a specific local model recommendation is clearly justified.

## Non-goals

- Do not add a new provider client implementation.
- Do not redesign provider authentication or model routing.
- Do not add dynamic model discovery at runtime.
- Do not remove user override support for `~/.config/insomnia/models.toml` / `providers.toml`.
- Do not change Profile authoring semantics.

## Acceptance criteria

- A short research note is recorded with provider sources and selected model IDs.
- `resources/models/builtin.toml` is updated to the current recommended set or explicitly confirmed current.
- Default profile model choice is confirmed or updated with rationale.
- Catalog parsing/merge tests pass.
- Validation includes at least `cargo test -p provider`, `cargo test -p manifest model`, `cargo fmt --check`, and `./tickets.sh doctor`.
