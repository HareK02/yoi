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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-17T09:50:53Z -->

## Implementation report

Implementation start failure / recovery note:

`queued -> inprogress` acceptance と worktree 作成は完了したが、Coder Pod spawn に失敗した。

Created worktree:
- `/home/hare/Projects/yoi/.worktree/00001KV5W3PHW-plugin-wasm-tool-runtime`
- branch: `impl/00001KV5W3PHW-plugin-wasm-tool-runtime`

Failure:
- `SpawnPod` failed before child Pod creation.
- Error: `failed to spawn /home/hare/Projects/yoi/target/debug/yoi (deleted) pod: No such file or directory (os error 2)`

Interpretation:
- Runtime binary path for this long-lived Orchestrator process points at a deleted executable. This is an operational/runtime issue, not a Ticket requirement issue.
- No Coder Pod was started and no implementation commit exists in the child worktree yet.

Recovery needed:
- Refresh/restart the Orchestrator runtime or provide a valid Pod runtime command so `SpawnPod` can launch children again.
- After recovery, reuse the existing child worktree/branch if clean and resume Coder spawn for this in-progress Ticket.

State handling:
- Ticket remains `inprogress` because acceptance and worktree side effects already happened; it is not silently returned to `queued`.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-18T12:05:59Z -->

## Implementation report

Implementation resumed:

Previous `SpawnPod` failure was recovered after Orchestrator runtime refresh. Reused existing clean child worktree and successfully spawned Coder Pod.

Worktree:
- `/home/hare/Projects/yoi/.worktree/00001KV5W3PHW-plugin-wasm-tool-runtime`
- branch: `impl/00001KV5W3PHW-plugin-wasm-tool-runtime`

Coder Pod:
- `yoi-coder-00001KV5W3PHW`

No duplicate `queued -> inprogress` transition was performed; this resumes the already accepted in-progress work.

---

<!-- event: implementation_report author: yoi-coder-00001KV5W3PHW at: 2026-06-18T12:31:01Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV5W3PHW`:

Commit:
- `10d12148 feat: run plugin tools through wasm runtime`

Changed files:
- `Cargo.lock`
- `crates/manifest/src/plugin.rs`
- `crates/pod/Cargo.toml`
- `crates/pod/src/feature/plugin.rs`
- `package.nix`

Implemented behavior:
- Added minimal sandboxed Plugin Tool WASM runtime using `wasmi`.
- Enabled registered Plugin Tool calls now execute through normal `Tool::execute` / `ToolOutput` / `ToolResult` paths.
- Added manifest-side helper to load resolved runtime module from pinned package:
  - validates runtime kind/ABI;
  - re-reads package from resolved path;
  - verifies deterministic package digest before selecting runtime entry;
  - bounds package/module size and reports safe diagnostics.
- Added `yoi-plugin-wasm-1` host ABI:
  - required export: `yoi_tool_call()`;
  - required guest export: `memory`;
  - host imports under `yoi:tool` only:
    - `tool_name_len() -> i32`
    - `tool_name_read(ptr, len) -> i32`
    - `input_len() -> i32`
    - `input_read(ptr, len) -> i32`
    - `output_write(ptr, len) -> i32`
- No WASI, filesystem, network, environment, or arbitrary host imports are provided; unsupported imports fail closed.

Bounds / failure behavior:
- Input JSON bound: `64 KiB`.
- Output JSON bound: `64 KiB`.
- Output summary bound: `1 KiB`.
- Linear memory bound: `2 MiB`.
- Table bound: `256`.
- Fuel bound: `5_000_000`.
- Wall timeout: `1s`.
- Diagnostics/errors are bounded and sanitized before surfacing as ordinary Tool errors.
- Malformed input JSON, malformed output JSON, output schema mismatch, oversized output, missing runtime/package/module, unsupported imports, load failure, and non-terminating execution fail closed.

Tests added/updated cover:
- Sample Plugin package Tool executes through WASM runtime.
- Tool input JSON reaches the module.
- Output JSON returns through ordinary Tool result path.
- Malformed input JSON.
- Malformed output JSON.
- Output schema mismatch.
- Oversize output.
- Non-terminating/high-fuel execution fail closed.
- Missing runtime/package/module diagnostic is safe and bounded.
- No ambient WASI/fs/network/env import access.
- Existing Plugin Tool registration behavior.

Validation reported by coder:
- Passed: `cargo fmt --check`
- Passed: `cargo check -p pod`
- Passed: `cargo test -p manifest plugin::`
- Passed: `cargo test -p pod feature::plugin`
- Passed: `git diff --check`
- Passed: `nix build .#yoi`
- Passed: `nix build .#yoi --no-link`

Residual risks / notes:
- ABI is intentionally minimal and not a general Plugin capability model.
- Permission grants, richer host APIs, WASI-like APIs, per-plugin runtime configuration knobs, and schema-driven rich output contracts remain follow-up work.
- Cancellation has no explicit `ToolExecutionContext` cancellation token today; this implementation provides safe timeout/fuel-bounded execution and maps timeout/join failure into ordinary Tool errors.

---
