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
