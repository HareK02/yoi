---
id: 20260603-001124-unify-reasoning-block-lifecycle
slug: unify-reasoning-block-lifecycle
title: Unify reasoning persistence with block lifecycle
status: open
kind: task
priority: P2
labels: [llm-worker, reasoning, timeline]
created_at: 2026-06-03T00:11:24Z
updated_at: 2026-06-03T00:11:24Z
assignee: null
legacy_ticket: null
---

## Background

Yoi currently has two reasoning-related event paths in `llm-worker`:

- streaming thinking/reasoning content is represented as block lifecycle events (`BlockStart` / `BlockDelta` / `BlockStop`);
- finalized reasoning persistence is represented separately as `ReasoningItemEvent` / `ReasoningItemCollector` and later converted into `Item::Reasoning`.

This duplicates the same conceptual model. Thinking and reasoning are provider vocabulary for the same class of generated model content, and generated content should be represented through the block lifecycle. `ReasoningItem` exists because the current reasoning block stop event does not carry the provider material needed for persistence, not because it is a distinct domain concept.

Backward compatibility is not required for this cleanup. Prefer a clear event model over compatibility aliases or parallel old/new APIs.

## Requirements

- Make the block lifecycle the authoritative path for reasoning/thinking generation and persistence.
- Remove the separate finalized reasoning side channel:
  - `ReasoningItemEvent`;
  - `ReasoningItemKind`;
  - `ReasoningItemCollector`;
  - `Timeline::on_reasoning_item` / `dispatch_reasoning_item` and related handler storage.
- Extend the reasoning/thinking block representation so block completion can carry all provider material required to build `Item::Reasoning`, including where applicable:
  - reasoning item id;
  - text;
  - summary;
  - encrypted content;
  - Anthropic thinking signature;
  - redacted thinking payload metadata.
- Update Anthropic streaming conversion so `thinking_delta` / `signature_delta` are accumulated in the block lifecycle and finalized through reasoning block stop metadata, without emitting a separate `ReasoningItem` event.
- Update OpenAI Responses conversion so completed reasoning items are represented as reasoning block lifecycle events, including metadata-only reasoning where there is no text delta.
- Update Worker collection/finalization so `Item::Reasoning` is built from reasoning block lifecycle state rather than `ReasoningItemCollector`.
- Preserve live streaming thinking/reasoning callbacks for UI/trace consumers.
- Preserve persisted reasoning history round-trip behavior for Anthropic and OpenAI Responses.
- Remove or rename misleading comments that imply reasoning is a meta/single event rather than generated content.
- Do not add backward-compatible aliases or keep the old `ReasoningItem` event path.

## Non-goals

- Do not redesign provider request serialization beyond what is needed to preserve existing reasoning round-trip behavior.
- Do not add a new E2E spawned-process test framework.
- Do not change public product behavior except for the internal event model cleanup.
- Do not hide missing provider material by dropping signatures, summaries, ids, or encrypted content.

## Acceptance criteria

- `ReasoningItemEvent` / `ReasoningItemCollector` and their Timeline handler path are removed.
- Anthropic thinking stream still produces live reasoning/thinking block callbacks and persists `Item::Reasoning` with signature/redacted material intact.
- OpenAI Responses reasoning items still persist and round-trip `id`, `summary`, and `encrypted_content` as before.
- Existing reasoning persistence tests are updated to assert the block-lifecycle path rather than the removed side channel.
- No compatibility aliases or duplicate reasoning concepts remain unless justified in the implementation report.
- Validation includes focused `llm-worker` reasoning tests, `cargo test -p llm-worker --lib`, `cargo check --workspace --all-targets`, `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` unless explicitly deferred with rationale.
