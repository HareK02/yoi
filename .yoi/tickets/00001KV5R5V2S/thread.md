<!-- event: create author: "yoi ticket" at: 2026-06-15T13:40:15Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T13:59:47Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T14:00:41Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- Prior Plugin package design `00001KT0Z4BK8` は done 済みで、本 Ticket はその設計を踏まえた discovery + explicit enablement resolver の最初の実装として具体化されている。
- Risk flags は plugin / package-loading / discovery / enablement / capability-boundary / startup-restore だが、non-goals と fail-closed / read-only / no-registration invariants が明確で、残る不確実性は typed module/config/resolver design の実装戦術に閉じている。

Evidence checked:
- Ticket body/thread: scope、requirements、non-goals、acceptance criteria、implementation notes、related work を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`425a6c66` 上。
- Visible Pods: implementation child Pod なし。
- Related design context: `00001KT0Z4BK8` done（Plugin package/discovery design）。

IntentPacket:

Intent:
- Plugin package discovery と explicit enablement resolver を typed module として実装し、package presence / discovery / enablement / runtime initialization / contribution registration を明確に分離する。

Binding decisions / invariants:
- Discovery は read-only。package の存在だけで execution / Tool / Hook / Service / Ingress registration を行わない。
- Explicit enablement entry がなければ Plugin は active にならない。
- Source-qualified identity (`user:<id>`, `project:<id>`, `builtin:<id>`) を扱い、ambiguous unqualified id は fail closed。
- Package safety checks は path traversal / root escape / bounded count/size / manifest size / deterministic digest を含む。
- unsupported/incompatible API version、digest mismatch、version mismatch、missing package、duplicate/ambiguous id、unsupported surface/grant は区別可能な diagnostic にする。
- Diagnostics を model-visible context に勝手に差し込まない。
- Plugin code execution / WASM runtime / actual Tool/Hook/Service/Ingress registration / MCP bridge は non-goal。
- No ambient workspace filesystem authority を plugin package discovery から発生させない。

Requirements / acceptance criteria:
- User store `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/*.yoi-plugin` と workspace store `<workspace>/.yoi/plugins/*.yoi-plugin` から package を発見できる。
- Valid package root の `plugin.toml` を parse し typed manifest と deterministic digest を得る。
- Invalid package は startup 全体を不要に壊さず bounded diagnostic で fail closed。
- Package without enablement is not active。
- Explicit enablement resolves package to typed resolved Plugin metadata。
- Tests cover valid user/workspace discovery、discovery-only inactive、explicit enablement、duplicate/ambiguous fail-closed、digest mismatch、path traversal/root escape、unsupported api、malformed manifest、no contribution registration。

Implementation latitude:
- Small typed module/crate-local module を追加してよい。runtime launch code に resolver logic を埋め込まない。
- `.yoi-plugin` archive vs directory minimal implementation は prior design に合わせる。必要なら最小サポート範囲を明示し後続拡張可能にする。
- Exact config/profile shape は既存 Profile / manifest design に合わせて最小 typed structure を追加してよい。
- Startup/restore reproducibility は deterministic re-resolution か resolved digest metadata 保持のどちらかを実装判断。ただし runtime-only mutable state 依存は不可。

Escalate if:
- Profile/manifest authority semantics、Pod restore semantics、secret handling、MCP enablement model を変える必要がある。
- Package archive implementation needs signature/trust/install/update/registry semantics。
- Arbitrary external filesystem/network authority が必要になる。
- Runtime registration/WASM execution なしでは acceptance を満たせないことが判明する。

Validation:
- focused tests for plugin discovery/resolver。
- `cargo fmt --check`。
- relevant `cargo check` / `cargo test`。
- `git diff --check`。
- `nix build .#yoi` if dependencies, runtime resources, packaging, or Cargo.lock changes matter。

Critical risks / reviewer focus:
- discovery vs enablement vs runtime/registration separation。
- fail-closed package safety / diagnostics。
- source-qualified identity and ambiguous refs。
- no contribution registration / no side effects from discovery。
- startup/restore determinism。
- secret-like diagnostic redaction。
- Plugin permission/grant requests not confused with actual grants。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T14:01:19Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、Plugin resolver work は同時に開始する Panel startup latency work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---
