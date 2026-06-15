<!-- event: create author: "yoi ticket" at: 2026-06-15T14:48:59Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T15:53:32Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T15:54:15Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。
- Outgoing dependency `00001KV5R5V2S` は `done` で、typed relation blocker は解消済み。
- 本 Ticket は resolved Plugin metadata を入力にした Tool surface registration boundary の実装であり、Plugin code execution / WASM runtime / permission grant enforcement は non-goal として明確。
- capability-boundary / model-visible-schema / tool-registry risk は高いが、acceptance criteria と fail-closed invariants が具体的で、残る不確実性は typed metadata / registry integration tactic に閉じている。

Evidence checked:
- Ticket body/thread: requirements、acceptance criteria、non-goals、related work を確認。
- Ticket relations: depends_on `00001KV5R5V2S` は done。incoming dependency from runtime Ticket `00001KV5W3PHW` は本 Ticket の blocker ではない。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`1fdb4cd6` 上。
- Visible Pods: implementation child Pod なし。

IntentPacket:

Intent:
- Enabled Plugin package の manifest Tool surface を読み取り、既存 `ToolRegistry` の model-visible schema 経路に安全に登録できる registration boundary を作る。ただし Tool call execution / WASM runtime はまだ実装しない。

Binding decisions / invariants:
- Discovery-only package は Tool schema surface に出さない。explicit enablement が必要。
- Tool registration は existing `ToolRegistry` 原則に従い、feature/profile config disabled なら model-visible schema から消える。
- Plugin Tool metadata に origin（plugin id/ref、source、digest、version/api、surface）を保持する。
- Duplicate Tool name は fail closed。builtin Tool / other Plugin Tool との衝突を曖昧に勝たせない。
- Invalid/unsupported input schema は fail closed。
- Runtime executor missing の Tool call は panic せず安全な unavailable/runtime-missing error を返す。
- Plugin code execution / WASM runtime / host API / permission grant enforcement / Service/Ingress/MCP bridge は non-goal。
- Permission declarations/grants を実効権限として扱わない。registration-time shape checks だけ。

Requirements / acceptance criteria:
- Enabled Plugin package の Tool definition が `ToolRegistry` に登録され、model-visible tools に現れる。
- Enablement がない Plugin package の Tool は model-visible tools に現れない。
- Duplicate Tool name / builtin collision は登録されず diagnostic で理由が分かる。
- Invalid input schema は登録されず diagnostic で理由が分かる。
- Registered Plugin Tool metadata から plugin origin / digest / source を追跡できる。
- Feature/profile flag により Plugin Tool surface を非表示にできる。
- Tool call が未実行状態でも panic せず unavailable/runtime-missing error。
- Tests cover enabled registration、no enablement inactive、duplicate Plugin Tool name、builtin collision、invalid schema、origin metadata、disabled feature/profile surface hiding。

Implementation latitude:
- Plugin manifest Tool definition schema の最小 shape は既存 design / resolver の strict parser に合わせてよい。
- Existing ToolRegistry contribution path に合わせた adapter / feature contribution module を追加してよい。
- Runtime-missing handler は minimal stub でよいが、model-visible registration と call failure behavior は明確にテストする。
- Diagnostics の storage/reporting surface は existing resolver diagnostics と整合させる。

Escalate if:
- ToolRegistry API が origin metadata を保持できず public API redesign が必要。
- Feature/profile config で Plugin Tool surface を安全に hide できない。
- JSON schema validation requires broader schema engine choice。
- Runtime-missing Tool stub が model/provider protocol に悪影響を与える。
- Permission grants / WASM executor / host API を実装しないと registration boundary が成立しない。

Validation:
- focused plugin/tool-registry tests。
- relevant `cargo test` / `cargo check`。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` if dependencies, Cargo.lock, package resources, or packaging correctness change。

Critical risks / reviewer focus:
- model-visible schema gating。
- duplicate/builtin collision fail closed。
- no enablement → no schema。
- origin metadata traceability。
- no execution/registration side effects beyond schema contribution。
- unavailable runtime call safety。
- permission declaration vs effective grant separation。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T15:54:24Z from: queued to: inprogress reason: orchestrator_acceptance_dependency_done field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。dependency `00001KV5R5V2S` は done で relation blocker は解消済み。Orchestrator workspace は clean。implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KV5W3PHA at: 2026-06-15T16:20:05Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV5W3PHA`:

Commit:
- `05a9c522 feat: register plugin tool surfaces`

Changed files:
- `crates/manifest/src/plugin.rs`
- `crates/manifest/src/lib.rs`
- `crates/manifest/src/config.rs`
- `crates/llm-worker/src/tool.rs`
- `crates/pod/src/feature.rs`
- `crates/pod/src/feature/plugin.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/pod.rs`

Implemented behavior:
- Added minimal Plugin package `tools` manifest surface parsing:
  - `name`
  - `description`
  - `input_schema`
- Added `feature.plugins` profile/manifest gate, default disabled.
- Added Plugin Tool feature adapter:
  - only enabled resolved Plugin packages with enabled Tool surface register schemas;
  - package discovery alone does not register Tools;
  - disabled profile feature suppresses Plugin Tool schema surface.
- Registered Plugin Tool definitions into existing ToolRegistry/model-visible schema path.
- Added host-side `ToolOrigin` metadata on `ToolMeta`:
  - plugin id/ref
  - source
  - digest
  - package version/API
  - `surface = "tool"`
- Added runtime-missing stub executor:
  - Tool schema is visible when registered;
  - actual calls return safe unavailable/runtime-missing error;
  - no Plugin code execution or WASM runtime added.
- Added registration-time fail-closed checks:
  - duplicate Plugin Tool names;
  - collision with already registered builtin Tool names;
  - invalid/unsupported input schemas.
- Adjusted feature install ordering so builtin/pending tools are flushed before Plugin feature registration, allowing Plugin collisions against already registered Tool names to be diagnosed instead of reaching later duplicate panic.

Diagnostics behavior:
- Duplicate Tool contributions produce feature install diagnostics and skip duplicate registration.
- Invalid Plugin input schema produces feature install diagnostics and registers no stub for that invalid Tool.
- Runtime calls before executor support return safe unavailable/runtime-missing `ToolError`.

Validation reported by coder:
- Passed: `cargo test -p manifest plugin --no-default-features`
- Passed: `cargo test -p pod plugin::tests --no-default-features`
- Passed: `cargo check -p pod -p manifest -p llm-worker`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`
- Attempted broader validation: `cargo test -p pod --no-default-features`
  - Failed in existing prompt text assertions unrelated to Plugin Tool surface implementation:
    - `prompt::tests::default_subagent_prompt_matches_resource`
    - `prompt::tests::subagent_prompt_treats_paths_as_data`

Not run:
- `nix build .#yoi` — no dependency, `Cargo.lock`, resource, or packaging changes.

Residual risks / blockers:
- Plugin executor is intentionally runtime-missing stub; actual WASM/runtime execution remains for later Ticket.
- Input schema validation is intentionally a narrow model-visible shape check, not a full JSON Schema engine. Unsupported composition/reference keywords are rejected fail-closed.

---

<!-- event: review author: yoi-reviewer-00001KV5W3PHA at: 2026-06-15T16:26:34Z status: request_changes -->

## Review: request changes

Review result: request_changes

Blocker:
- Invalid nested Plugin Tool schemas can still become model-visible.

Evidence:
- `crates/pod/src/feature/plugin.rs` validates root-level schema shape and recursively rejects only selected unsupported keywords.
- It does not validate that nested schema nodes are valid JSON Schema objects.
- Example invalid schema that appears to pass current validation:

```toml
input_schema = {
  type = "object",
  properties = { query = "not-a-schema" },
  required = ["query"],
  additionalProperties = false
}
```

Why this is a blocker:
- `properties.query = "not-a-schema"` is not a valid schema node.
- Current checks can allow it into ToolRegistry / model-visible tool schema path.
- This violates the acceptance criteria that invalid input schema is rejected with diagnostic and the critical focus on model-visible schema gating.

Required changes:
- Strengthen Plugin Tool schema validation before registration so invalid nested schemas fail closed with diagnostics.
- Add regression tests showing nested invalid schema nodes do not register a Plugin Tool.
- Prefer defining/enforcing an explicit supported subset recursively, for example:
  - validate property schemas;
  - validate allowed `type` values;
  - validate `required` string/unique array;
  - validate or reject `items` / `additionalProperties` recursively;
  - reject unknown/problematic keywords unless explicitly supported.

Positive evidence checked:
- Plugin tool registration is feature-gated by `feature_config.plugins.enabled`.
- Discovery-only packages do not register tools because registration uses resolved enabled Plugin records/surfaces.
- Registered Plugin Tools preserve `ToolOrigin::Plugin` metadata.
- Builtin collisions and duplicate Plugin Tool names are diagnosed/rejected.
- Runtime execution is a safe unavailable/runtime-missing stub.
- No Plugin code execution, WASM runtime, host API, Service/Ingress/MCP bridge, or permission grant enforcement was added.
- `ToolOrigin` is not exposed directly to model-visible schema.

Validation performed by reviewer:
- Passed: `git diff --check fcae8860..05a9c522`
- Passed: `cargo fmt --check`
- Worktree remained clean.

Validation not run:
- `cargo test` / `cargo check` were not rerun because reviewer scope was read-only.

Conclusion:
- Changes requested. Do not integrate until nested schema validation is fail-closed and covered.

---

<!-- event: implementation_report author: yoi-coder-00001KV5W3PHA at: 2026-06-15T16:30:49Z -->

## Implementation report

Follow-up implementation report after reviewer request_changes:

New commit:
- `3413bae7 fix: reject nested plugin tool schema errors`

Changed file:
- `crates/pod/src/feature/plugin.rs`

Schema validation behavior updated:
- Replaced shallow root-only validation with recursive validation of an explicit supported subset.
- Every schema node must be a JSON object with supported string `type`.
- Supported `type` values:
  - `object`
  - `array`
  - `string`
  - `number`
  - `integer`
  - `boolean`
  - `null`
- Supported keywords:
  - `type`
  - `title`
  - `description`
  - `properties`
  - `required`
  - `additionalProperties`
  - `items`
  - `enum`

Recursive/fail-closed behavior:
- `properties.*` values are recursively validated as schema objects.
- `items` is only allowed on array schemas and recursively validates a schema object.
- `additionalProperties` is only allowed on object schemas and must be boolean or recursively valid schema object.
- Non-object nested schema nodes reject registration.
- Unknown/problematic keywords reject registration.
- Unsupported `type` values reject registration.
- `required` must be an array of unique strings.
- `required` entries must be declared in `properties` when `properties` exists.
- `enum` must be non-empty array with unique entries.

Regression tests added:
- Direct validation rejects:
  - `properties.query = "not-a-schema"`
  - invalid recursive `items`
  - invalid recursive `additionalProperties`
  - duplicate `required` entries
- Registration test confirms nested invalid schema does not register a Plugin Tool and emits an `invalid input_schema` diagnostic with nested path.

Validation reported by coder:
- Passed: `cargo test -p pod plugin::tests --no-default-features`
- Passed: `cargo test -p manifest plugin --no-default-features`
- Passed: `cargo check -p pod -p manifest -p llm-worker`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`

Repository status:
- Child implementation worktree clean after follow-up commit.

---
