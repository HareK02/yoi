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
