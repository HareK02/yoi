<!-- event: create author: "yoi ticket" at: 2026-06-15T14:48:59Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-17T09:46:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-17T09:48:31Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relations / OrchestrationPlan / Orchestrator workspace state を確認した。
- `depends_on 00001KV5W3PHA` は `closed/done` 済みで、typed relation blocker は解消済み。
- Incoming dependent `00001KV5W3PJ3` は本 Ticket の blocker ではなく、runtime path の grant enforcement 後続として扱う。
- 本 Ticket は minimal WASM runtime による Plugin Tool execution の最初の実装であり、host API / fs / https / Service / Ingress / MCP / long-running process を明確に non-goal としている。
- sandbox / tool-runtime / capability-boundary / cancellation risk は高いが、timeout、bounds、ambient authorityなし、structured error、deterministic module selection が Ticket に明記されているため、implementation_ready と判断する。

Evidence checked:
- Ticket body/thread: requirements、acceptance criteria、non-goals、validation、risk flags を確認。
- Ticket relations: outgoing `depends_on 00001KV5W3PHA` は done/closed。related design `00001KSXRQ4G8` は blocker ではない。incoming `00001KV5W3PJ3` は dependent。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`bcb8068e` 上。
- Visible Pods: implementation child Pod なし。

IntentPacket:

Intent:
- Registered/enabled Plugin Tool call を minimal sandboxed WASM runtime に route し、bounded input/output/error と通常 Tool history 経路で安全に結果を返す。

Binding decisions / invariants:
- Runtime は ambient filesystem / network / environment authority を持たない。
- Host API imports は tool input/output に必要な最小限のみ。`fs` / `https` は実装しない。
- Plugin stdout/stderr相当や raw memory dump を無制限に history/model-visible context に入れない。
- Tool call/result は通常 Tool history 経路を使い、hidden context injection をしない。
- Timeout / cancellation / input size / output size / diagnostic size bounds を実装する。
- Malformed JSON / schema mismatch / oversize output / non-terminating execution は fail closed。
- Runtime module selection は package digest/runtime config に基づき deterministic。runtime-only mutable state に依存しない。
- Permission grants / host API authority / fs/network は後続 Ticket。ここでは minimal no-authority runtime execution のみ。

Requirements / acceptance criteria:
- Enabled Plugin Tool invocation が Plugin runtime に route される。
- Minimal WASM module load、tool input JSON delivery、tool output JSON receipt、structured error handling が実装される。
- Ambient authority なしで実行される。
- Bounds と timeout/cancellation が効く。
- Invalid output は safe Tool error。
- Successful Plugin Tool result は通常 Tool result として返る。
- Runtime missing/malformed module/load failure は safe structured Tool error。
- Tests cover success、malformed output、oversize output、timeout/cancellation、input bound、missing module、no ambient fs/network/env、normal Tool history path。

Implementation latitude:
- WASM runtime crate/engine selectionは既存 deps/packaging constraintsに合わせる。新 dependency が必要なら `Cargo.lock` / `package.nix` / `nix build .#yoi` を更新・検証する。
- Guest ABI は minimal JSON input/output ABI でよい。将来拡張可能に docs/code comment で境界を明示する。
- Real package archive fixture or synthetic resolved plugin fixture は実装判断。ただし deterministic digest/module selection は test で固定する。

Escalate if:
- WASM engine dependency / sandbox model が project-wide design decision を要する。
- Cancellation/timeout cannot be made reliable without broader runtime changes。
- Host API import semantics must be added beyond input/output。
- ToolRegistry / Worker protocol needs broad API change。
- Security boundary cannot be enforced within existing process model。

Validation:
- focused plugin WASM runtime tests。
- existing Plugin Tool registration tests。
- `cargo check` / relevant `cargo test`。
- `cargo fmt --check`。
- `git diff --check`。
- `nix build .#yoi` if dependencies/Cargo.lock/package changes occur。

Critical risks / reviewer focus:
- actual sandbox/ambient authority isolation。
- timeout/cancellation reliability。
- bounded outputs/errors/history。
- deterministic module selection from resolved plugin metadata。
- no hidden context injection。
- runtime missing/load/invalid-output safe errors。
- dependency/packaging correctness。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-17T09:49:19Z from: queued to: inprogress reason: orchestrator_acceptance_dependency_done field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。dependency `00001KV5W3PHA` は done/closed で relation blocker は解消済み。Plugin WASM runtime work は同時開始する Panel E2E readiness work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---
