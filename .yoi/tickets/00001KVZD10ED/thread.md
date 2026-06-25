<!-- event: create author: "yoi ticket" at: 2026-06-25T12:45:38Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T13:23:50Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T13:23:50Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T13:24:26Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T13:26:00Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は `llm-worker` / `llm-worker-macros` crate rename、Rust import path rename、major public type rename (`Worker` -> `Engine`, config/result/error names)、repo-wide references、validation を具体的に列挙している。
- `TicketRelationQuery` は blocking relation 0 件、`TicketOrchestrationPlanQuery` は routing 前 plan 0 件だった。
- bounded context check で repository-wide `llm-worker` / `llm_worker` references を確認し、主な変更 surface は `crates/llm-worker`, `crates/llm-worker-macros`, workspace/dependency metadata, `pod`/`manifest` imports, docs/tests/examples/Nix/Cargo.lock である。これは大きめだが mechanical rename task として明確で、責務移動や WorkerRuntime 実装は non-goal として分離されている。
- risk は API/naming churn だが、受け入れ条件と validation が明確で、Coder の bounded implementation latitude に収まる。planning return が必要な未決定事項は見つからない。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。thread は create、planning->ready、ready->queued のみで未解決 blocker は記録されていない。
- Relations / orchestration plan: relation 0 件、routing 前 plan 0 件。accepted plan `orch-plan-20260625-132518-1` を記録済み。
- Code context: `git grep` で `crates/llm-worker`, `crates/llm-worker-macros`, `llm_worker`, `llm_worker_macros`, `Worker` imports/examples/tests/docs references を確認。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。queued Ticket はこの 1 件、inprogress Ticket は 0 件。

IntentPacket:

Intent:
- LLM turn-processing crate を `llm-worker` から `llm-engine` へ rename し、public turn-engine主体型を `Worker` から `Engine` 系へ rename することで、今後の Runtime-scoped Worker concept と衝突しない package/API naming に整理する。

Binding decisions / invariants:
- `llm-engine` は LLM turn engine。Runtime / Worker identity / process lifecycle / socket protocol / session file authority は持たない。
- 責務移動は最小限。provider request/stream handling、tool-call loop、reasoning/usage/retry/continuation/history/callback semantics は変えない。
- `crates/llm-worker` / `crates/llm-worker-macros` は残さない。
- `llm-worker` / `llm_worker` / `llm_worker_macros` compatibility alias は作らない。
- `pod` crate and dependents should compile against `llm_engine::Engine` and renamed config/result/error types.
- New `worker-runtime` crate or Pod->Worker migration is non-goal.

Requirements / acceptance criteria:
- `crates/llm-engine` and `crates/llm-engine-macros` exist; old directories gone.
- Cargo package/dependency names and Rust import paths use `llm-engine` / `llm_engine` and `llm-engine-macros` / `llm_engine_macros`.
- Public turn-engine type is `llm_engine::Engine`, not `Worker`; config/result/error names no longer use `Worker` for the turn engine concept.
- Repository-wide old references are gone except intentional historical/migration notes if any.
- `pod`, `manifest`, `yoi`, examples/tests/docs/Nix/Cargo.lock update consistently.
- Existing runtime behavior is unchanged except names.
- Validation target includes `cargo test -p llm-engine`, `cargo test -p pod`, `cargo check -p yoi`, `git diff --check`, `nix build .#yoi --no-link`.

Implementation latitude:
- Result/output type exact names may follow Ticket guidance (`EngineRunResult`, `EngineRunOutput`, etc.) as long as public API no longer presents LLM turn engine as Worker.
- Internal file names may be renamed for clarity (`worker.rs` -> `engine.rs`) if practical; otherwise public module/API must be clean.
- Historical ticket ids or changelog-like references may remain only if clearly intentional and not active API/docs guidance.

Escalate if:
- Rename requires behavior changes to provider streaming/tool-loop/history semantics.
- A compatibility alias appears necessary to make internal crates compile.
- Existing macro/test generated names cannot be renamed without broader procedural macro redesign.
- `cargo test -p pod` failure is not the known prompt guidance snapshot caveat but a rename regression.

Validation:
- `cargo test -p llm-engine`
- `cargo test -p pod`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Add focused grep evidence for old names.

Current code map:
- Primary: `crates/llm-worker`, `crates/llm-worker-macros`, workspace `Cargo.toml`, `Cargo.lock`, `package.nix`。
- Secondary: dependent imports in `crates/pod`, `crates/manifest`, examples/tests/docs/resources as found by grep。
- Avoid: worker-runtime implementation, Pod runtime migration, session/socket authority changes, provider/tool-loop behavior changes。

Critical risks / reviewer focus:
- stale old crate/import/type references。
- hidden compatibility alias left behind。
- behavior changes mixed into mechanical rename。
- `Worker` remaining as public turn-engine主体名 where it conflicts with Runtime Worker concept。
- Nix/Cargo lock/hash updates missed。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVZD10ED-llm-engine-rename` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T13:26:35Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、blocking relation は 0 件、accepted plan `orch-plan-20260625-132518-1` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- orchestration worktree は clean。この Ticket 用 worktree/branch はまだ作成していない。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVZD10ED-llm-engine-rename` を作成し、multi-agent-workflow に接続する。

---
