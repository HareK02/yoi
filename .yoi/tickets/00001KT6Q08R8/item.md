---
title: "Hook: harden public hook surface before plugin exposure"
state: "closed"
created_at: "2026-06-03T12:23:17Z"
updated_at: "2026-06-03T17:07:44Z"
---

## Issue

`crates/pod/src/hook.rs` is already positioned as the public orchestration extension point over the internal `llm_worker::Interceptor`, but actual production usage is still light. Current usage is mostly manifest permission policy (`PreToolCall`) and usage tracking (`PreLlmRequest`). Before external or plugin-like features rely on Hooks, the public Hook surface needs tighter safety boundaries and better Pod-layer test coverage.

The main concern is that some Hook outputs still reuse internal `llm_worker::interceptor` action types. In particular, `PreRequestAction::ContinueWith(Vec<Item>)` and `TurnEndAction::ContinueWithMessages(Vec<Item>)` allow model-visible item injection from a Hook path. That conflicts with the intended public rule that Hooks must not mutate history/context except through approved durable paths.

## Requirements

- Audit the public Hook API against the intended plugin/feature contract.
- Replace or wrap internal interceptor action types with safe public Hook action subsets where needed.
  - `OnPromptSubmit` already has a safe `HookPromptAction`; use the same pattern for other public Hook events.
  - Public Plugin/Feature Hooks should not be able to return raw `Item` vectors for hidden context/history mutation.
- Preserve internal mechanisms that legitimately need richer interceptor actions, but keep them separate from public feature/plugin Hooks.
- Keep manifest permission policy behavior intact.
  - Deny/ask still fails closed with synthetic tool results as today.
- Clarify which Hook events are observational only and which can cancel/abort/yield/deny.
- Add Pod-layer tests for Hook behavior that is currently only lightly covered.

## Suggested test coverage

- `OnPromptSubmit` Hook can cancel a submitted prompt through the safe public action type.
- `PreLlmRequest` public Hook runs under normal conditions and cannot inject raw `Item` context.
- `PreToolCall` Hook can deny/skip with a synthetic result through the intended path.
- `PostToolCall` Hook can observe a bounded summary and abort when allowed, but cannot rewrite tool output unless an explicit safe transform capability is introduced.
- `OnTurnEnd` public Hook cannot append arbitrary messages through raw internal action types.
- `OnAbort` Hook is called on abort paths.
- Multiple Hooks execute in registration order and short-circuit on non-continue actions.
- Internal mechanisms run in the expected order relative to public Hooks.

## Non-goals

- Implementing a Plugin runtime.
- Implementing a feature registry.
- Adding declarative hooks.
- Adding new Hook event kinds unless a gap is found during the audit.
- Allowing Hooks to rewrite arbitrary history/tool/model context.

## Acceptance criteria

- Public `pod::hook` APIs no longer expose internal action types that allow raw model-visible `Item` injection.
- Internal interceptor capabilities remain available only through internal mechanisms where justified.
- Existing manifest permission policy and usage tracking behavior remain intact.
- Focused Pod-layer tests cover the Hook events and short-circuit behavior needed by the feature registry/plugin surface.
- The resulting Hook surface is safe to reference from `plugin-feature-contribution-registry` as the public Hook contribution boundary.
