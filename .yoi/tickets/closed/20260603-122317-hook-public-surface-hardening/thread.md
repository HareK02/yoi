<!-- event: create author: tickets.sh at: 2026-06-03T12:23:17Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-03T16:36:57Z -->

## Plan

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


---

<!-- event: review author: hare at: 2026-06-03T16:58:49Z status: request_changes -->

## Review: request changes

# External review: hook public surface hardening

## 1. Result: request changes

Request changes. The implementation largely moves prompt/request/turn-end hook actions behind public wrapper types and preserves the internal `llm_worker::Interceptor` action model, but one public pre-tool action still exposes the unsafe internal skip semantics instead of the ticketed fail-closed/synthetic-result behavior.

## 2. Summary of implementation

The coder introduced a public `pod::hook` action surface with event-specific wrapper actions:

- `HookPreRequestAction` and `HookTurnEndAction` expose `Continue`, `Abort`, and bounded textual prompt actions instead of raw `llm_worker::Item` continuation actions.
- `HookPreToolAction` exposes `Continue`, `Skip`, `Deny`, `Pause`, and `Abort`, with `Deny` carrying a public message string that is converted into an internal synthetic tool result.
- `HookPostToolAction` exposes only `Continue` and `Abort`.
- `PodInterceptor` now adapts public hook outputs to the richer internal `llm_worker::Interceptor` actions, so internal code can still use `PreRequestAction::ContinueWith`, `TurnEndAction::ContinueWithMessages`, and synthetic `ToolResult` construction where needed.
- Permission policy was adapted onto `HookPreToolAction::Deny`, preserving synthetic denial results for `deny` and fail-closed `ask`.

## 3. Requirement-by-requirement assessment

- Public `pod::hook` surface no longer exposes raw request/turn continuation injection: mostly satisfied. I did not find a public re-export or alias of `PreRequestAction`, `TurnEndAction`, raw `Item`, or arbitrary `ToolResult` construction through `pod::hook`. Public pre-request and turn-end hooks can only emit textual prompt actions plus continue/abort, and the raw `Item` conversions remain internal to the adapter.
- Internal mechanisms that need richer `llm_worker::Interceptor` actions remain internal: satisfied. The bridge still maps public prompt actions into internal `PreRequestAction::ContinueWith` / `TurnEndAction::ContinueWithMessages`, and compact/internal interceptors can keep using the richer worker-level API.
- Manifest permission policy fail-closed behavior: satisfied for deny/ask. `PolicyDecision::Deny` and `PolicyDecision::Ask` both convert to public `HookPreToolAction::Deny`, and the bridge converts that to internal `PreToolAction::SyntheticResult` with `is_error = true`.
- Public hooks cannot invisibly mutate prompt context/history: not fully satisfied. Pre-request and turn-end prompt mutations are explicit textual hook actions, but public pre-tool `Skip` still maps to the internal no-result skip path; see blocker below.
- Public hook names/types are usable for a future feature/plugin API: broadly satisfied. The event-specific `Hook*Action` types are clearer than exporting internal worker actions. One follow-up API tightening is noted below.
- No unnecessary compatibility aliases or broad refactors: satisfied. The diff is limited to the hook bridge, permission adapter, `Pod` registration plumbing, and tests.
- Tests cover public hook behavior and short-circuit ordering: partially satisfied. Added tests cover pre-request/turn-end public prompt actions, pre-request abort short-circuiting, pre-tool deny synthetic result, post-tool abort, and registration ordering. They do not cover the public `Skip` behavior required by the ticket.

## 4. Blockers

### Blocker: public `HookPreToolAction::Skip` keeps the internal no-result skip semantics

`crates/pod/src/hook.rs` exposes `HookPreToolAction::Skip` as a public action, documented as skipping the tool call without executing it, and converts it directly to `llm_worker::interceptor::PreToolAction::Skip`. In `llm-worker`, `PreToolAction::Skip` removes the call from the approved tool list and does not construct a synthetic `ToolResult`.

That conflicts with the ticket/delegation requirement that public pre-tool hooks can deny/skip only through the intended synthetic-result path. It also means a public feature/plugin hook can cause a model-emitted tool call to have no corresponding tool result, which is an invisible conversation/tool-history mutation and can break the assistant/tool-call pairing expected by later model requests.

Required fix before merge: either remove public `Skip`, or define the public skip/deny path as a synthetic error/result action rather than mapping to internal `PreToolAction::Skip`. Add a test that the public skip/deny path produces a synthetic tool result and preserves tool-call/result pairing or, if public skip is intentionally unsupported, that the public API cannot express it.

## 5. Non-blockers / follow-ups

- `ToolResultSummary` still exposes `llm_worker::tool::ToolOutput` as part of the public hook API. This does not allow arbitrary `ToolResult` construction or output rewriting, so I am not treating it as a merge blocker for this ticket. For the plugin/feature registry API, a dedicated bounded public summary type would reduce coupling to `llm_worker` internals.
- The current public action names are mechanically clear but somewhat verbose (`HookPreRequestAction`, `HookTurnEndAction`, etc.). They are acceptable for this hardening step; any naming polish can happen as part of the broader registry/API design.

## 6. Validation assessed or rerun

Rerun/read-only checks from `/home/hare/Projects/yoi/.worktree/hook-public-surface-hardening`:

- `git status --short` was clean.
- `git rev-parse HEAD` reported `2f020ed0bb3b9487ebc4671afd969ee5c8727cfa`.
- `git diff --check develop...HEAD` passed.
- `cargo fmt --check` passed.
- `./tickets.sh doctor` passed.

Assessed by source review:

- `git diff develop...HEAD`
- `crates/pod/src/hook.rs`
- `crates/pod/src/ipc/interceptor.rs`
- `crates/pod/src/permission.rs`
- `crates/pod/src/pod.rs`
- relevant `llm-worker` pre-tool handling

I did not run `cargo test`, `cargo check`, or `nix build`; those commands would write build artifacts outside this review artifact scope. Existing tests were reviewed from the diff/source instead.

## 7. Residual risk

After fixing the public skip path, the main remaining risk is API shape stabilization for the future plugin/feature registry: public hook inputs still share some worker/tool data types, and those should be deliberately frozen or wrapped before becoming a long-term plugin ABI. The raw `Item` injection risk for pre-request and turn-end hooks appears addressed by this implementation.


---

<!-- event: review author: hare at: 2026-06-03T17:05:43Z status: approve -->

## Review: approve

# External rereview: hook public surface hardening

## 1. Result: approve

Approve. The previous blocker is fixed: the public pre-tool hook action surface no longer exposes a no-result skip action, and public deny still maps to a synthetic error result.

## 2. Summary of rereviewed changes

The follow-up commit `a4e30e2 fix: remove public hook skip action` narrowed `HookPreToolAction` by removing the `Skip` variant and its conversion to `llm_worker::interceptor::PreToolAction::Skip`. The public pre-tool actions are now `Continue`, `Deny(String)`, `Abort(String)`, and `Pause`.

`HookPreToolAction::Deny` still converts internally to `PreToolAction::SyntheticResult(ToolResult::error(call_id, reason))`, preserving the fail-closed/synthetic-result path needed by manifest permissions and future public hooks.

## 3. Requirement-by-requirement assessment

- Previous blocker fixed: satisfied. `HookPreToolAction::Skip` is gone, and grep found no `HookPreToolAction::Skip`, `PreToolAction::Skip`, or standalone `Skip` usage under `crates/pod/src`.
- Public pre-tool deny maps to synthetic error result: satisfied. `HookPreToolAction::Deny` converts to `PreToolAction::SyntheticResult(ToolResult::error(...))`; existing interceptor tests also assert the synthetic result has the expected call id, summary, no content, and `is_error = true`.
- Public Hook API exposes no no-result skip: satisfied. The only no-result skip capability remains in the lower-level `llm_worker` interceptor model; it is no longer reachable through the public `pod::hook::HookPreToolAction` surface.
- Public Hook API exposes no raw `Item` injection: satisfied. `PreLlmRequest` and `OnTurnEnd` use safe public action types rather than raw `PreRequestAction::ContinueWith(Vec<Item>)` or `TurnEndAction::ContinueWithMessages(Vec<Item>)` as hook outputs.
- Public Hook API exposes no arbitrary `ToolResult` construction: satisfied for action outputs. Public pre-tool hooks provide only a denial message; Pod constructs the synthetic error result internally.
- Internal capabilities remain internal: satisfied. Internal Pod/Worker code still uses richer `llm_worker::Interceptor` actions where needed, including durable host-owned prompt append paths and compact/internal mechanisms.
- Tests cover the fixed path sufficiently: satisfied. The added `public_pre_tool_hook_actions_cannot_emit_internal_no_result_skip` unit test verifies the available public pre-tool conversions and asserts public deny produces a synthetic result. The existing `public_pre_tool_hook_deny_becomes_synthetic_error_and_short_circuits` integration-style interceptor test covers bridge behavior and ordering.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- As noted in the original review, `ToolResultSummary` still exposes `llm_worker::tool::ToolOutput` as public hook input. This is not a blocker because it does not allow output rewriting or arbitrary `ToolResult` construction, but the future plugin/feature registry may still want a Pod-owned summary type before freezing a public API boundary.

## 6. Validation assessed or rerun

Rerun/read-only checks from `/home/hare/Projects/yoi/.worktree/hook-public-surface-hardening`:

- `git rev-parse HEAD` reported `a4e30e292abf5c640b923e3307a75eded366351a`.
- `git status --short` was clean.
- `git diff --check develop...HEAD` passed.
- `cargo fmt --check` passed.
- `./tickets.sh doctor` passed.
- `git grep -n -E 'HookPreToolAction::Skip|PreToolAction::Skip|pub (use|type).*\b(PreRequestAction|TurnEndAction|PreToolAction|PostToolAction|ToolResult|Item)\b' crates/pod/src` produced no matches.

Assessed by source review:

- Follow-up commit `a4e30e2` diff.
- Full diff `develop...HEAD` for `crates/pod/src/hook.rs`, `crates/pod/src/ipc/interceptor.rs`, `crates/pod/src/permission.rs`, and `crates/pod/src/pod.rs`.
- Relevant lower-level `llm-worker` pre-tool handling to confirm internal skip remains lower-level only.

I did not run `cargo test`, `cargo check`, or `nix build`; those commands would write build artifacts outside this review artifact scope. The fixed-path tests were assessed from source.

## 7. Residual risk

No merge-blocking residual risk found. The remaining risk is API polish for the future feature/plugin registry, especially whether all hook input summary types should become Pod-owned wrapper types before being treated as a stable plugin boundary.


---

<!-- event: close author: hare at: 2026-06-03T17:07:44Z status: closed -->

## Closed

Hardened public pod::hook actions for plugin/feature exposure. Public hooks no longer expose raw Item injection or no-result pre-tool skip; public pre-tool deny maps to synthetic error results; internal interceptor capabilities remain internal. Added focused Pod-layer hook tests; reviewer blocker fixed and rereview approved. Validation passed: cargo test -p pod hook --lib, cargo test -p pod --lib, cargo test -p llm-worker --lib, cargo fmt --check, cargo check --workspace --all-targets, ./tickets.sh doctor, git diff --check, nix build .#yoi, ./result/bin/yoi pod --help.


---
