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
