# Delegation intent: Hook public surface hardening

## Intent

Harden `pod::hook` so it can be safely used as the public Hook contribution boundary for the feature/plugin registry. Public Hooks must not expose internal `llm_worker::Interceptor` action types that can inject raw model-visible `Item` values into request/history paths.

## Requirements

- Audit the current public Hook API in `crates/pod/src/hook.rs` and its bridge in `crates/pod/src/ipc/interceptor.rs`.
- Replace or wrap public Hook outputs that currently reuse internal interceptor action types with safe public action subsets.
  - `OnPromptSubmit` already uses `HookPromptAction`; use the same pattern for events that need public actions.
  - Public Hook APIs must not expose raw `Item` vector injection such as `PreRequestAction::ContinueWith(Vec<Item>)` or `TurnEndAction::ContinueWithMessages(Vec<Item>)`.
- Preserve internal mechanisms that legitimately need richer `llm_worker::Interceptor` actions, but keep them internal and separate from public feature/plugin Hooks.
- Preserve current manifest permission policy behavior.
  - `PreToolCall` deny/ask still fails closed through the existing synthetic tool result path.
- Preserve usage tracking behavior.
- Clarify through names/types/tests which Hook events are observation-only and which can cancel/abort/yield/deny.
- Add focused Pod-layer tests for public Hook behavior and short-circuit ordering.

## Invariants

- Do not add hidden prompt/context injection paths.
- Do not mutate session history from public Hooks except through already-approved durable host paths.
- Do not expose `llm_worker::Item` or raw history/message vectors through public plugin/feature Hook actions.
- Do not implement plugin runtime, feature registry, MCP, or WorkItem tools in this ticket.
- Do not weaken manifest permission enforcement.
- Keep `llm_worker::Interceptor` internal capabilities available where currently required by Pod internals.

## Non-goals

- Implementing `plugin-feature-contribution-registry`.
- Adding new Hook event kinds unless the audit finds a strict safety gap.
- Allowing Hooks to rewrite tool outputs or arbitrary model context.
- Broad refactors of Pod/Worker runtime unrelated to the Hook surface.

## Suggested files to inspect

- `crates/pod/src/hook.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/permission.rs`
- `crates/pod/src/pod.rs`
- `crates/llm-worker/src/interceptor.rs`
- `crates/llm-worker/tests/parallel_execution_test.rs`

## Known observations from pre-delegation investigation

- Current production Hook use is light:
  - `PermissionHook` implements `Hook<PreToolCall>` in `crates/pod/src/permission.rs`.
  - `UsageTrackingHook` uses `PreLlmRequest` from `crates/pod/src/pod.rs`.
- `OnPromptSubmit` already has a safe public subset action: `HookPromptAction`.
- `PreLlmRequest` and `OnTurnEnd` currently expose internal action types with raw `Item` injection capability.
- `PostToolCall` is currently observation + abort only and does not rewrite tool output; keep that conservative unless a strictly bounded explicit transform is justified, which is not expected for this ticket.
- Existing `llm-worker` interceptor tests cover some lower-level behavior, but Pod-layer Hook coverage should be improved.
- `cargo test -p pod hook --lib` passed during investigation.
- Individual relevant `llm-worker` interceptor tests passed, but the full `parallel_execution_test` file had an unrelated timing-sensitive failure (`test_parallel_tool_execution` took ~1.37s instead of ~100ms). Do not treat that file-wide failure as a Hook blocker without confirming.

## Validation

Run at least:

- `cargo test -p pod hook --lib`
- focused Pod Hook tests added/updated by this ticket
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

If broader validation fails due to pre-existing unrelated timing flakes, report exact command/output and run focused commands that isolate this change.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- public Hook API changes
- internal mechanism separation
- tests added/updated
- validation commands and results
- unresolved risks or follow-up recommendations
- whether the work is ready for external review
