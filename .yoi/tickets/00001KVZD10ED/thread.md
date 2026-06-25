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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T13:28:20Z -->

## Implementation report

Implementation orchestration update:

- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVZD10ED-llm-engine-rename` と branch `work/00001KVZD10ED-llm-engine-rename` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVZD10ED-llm-engine` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVZD10ED-llm-engine at: 2026-06-25T13:47:26Z -->

## Implementation report

Implementation report:

対象 Ticket:
- `00001KVZD10ED` — `llm-worker crateをllm-engineへ改名する`

実装概要:
- `crates/llm-worker` → `crates/llm-engine`
- `crates/llm-worker-macros` → `crates/llm-engine-macros`
- Cargo package / workspace dependency 名を `llm-engine` / `llm-engine-macros` へ更新。
- Rust import path を `llm_engine` / `llm_engine_macros` へ更新。
- turn-engine API を rename。
  - `Worker` → `Engine`
  - `WorkerConfig` → `EngineConfig`
  - `WorkerError` → `EngineError`
  - `WorkerResult` → `EngineResult`
  - `WorkerState` → `EngineState`
  - `RunOutput` → `EngineRunOutput`
- `pod` 側の engine 保持 API / error / alert source 等も rename fallout として更新。
- docs / tests / examples / Nix / `Cargo.lock` / `package.nix` hash を更新。
- 旧 crate directory は削除済みで、互換 alias は追加していない。

Commit:
- `292fc4ea refactor: rename llm worker crate to engine`

Validation:
- `cargo test -p llm-engine`: success
- `cargo test -p pod`: success
  - 既知 caveat の prompt guidance snapshot failure は今回発生しなかった。
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Grep evidence:
- `.yoi/tickets/**` と `docs/report/**` を除外した active tree で以下の旧 active references は 0 件。
  - `llm-worker`
  - `llm_worker`
  - `llm-worker-macros`
  - `llm_worker_macros`
- `crates/llm-engine` / `crates/llm-engine-macros` 内の旧 public Worker API 名も 0 件。
  - `\bWorker\b`
  - `WorkerConfig`
  - `WorkerError`
  - `WorkerResult`
  - `WorkerState`
- historical `.yoi/tickets` / `docs/report` 内には旧語が残る。count: 786。これは Ticket 本文・過去 report 類の履歴文脈として意図的に残している。

Residual risks / notes:
- 互換 alias は入れていないため、外部未追跡コードが `llm-worker` / `llm_worker::Worker` を直接参照している場合は破壊的変更になる。ただし Ticket の binding invariant 通り。
- merge / Ticket close / worktree cleanup は未実施。

---

<!-- event: review author: yoi-reviewer-00001KVZD10ED-llm-engine at: 2026-06-25T13:54:15Z status: approve -->

## Review: approve

Review result: approve

確認結果、Ticket `00001KVZD10ED` の IntentPacket / acceptance criteria に対してブロッカーは見つかりませんでした。

Read-only 確認:
- 対象 worktree / branch / commit:
  - `work/00001KVZD10ED-llm-engine-rename`
  - HEAD `292fc4ea`
  - base `22598710` は ancestor。
- Ticket 記録:
  - `thread.md` の IntentPacket / acceptance criteria / reviewer focus を確認。
  - 要件は `llm-worker` → `llm-engine`、public turn-engine API を `Worker` 系から `Engine` 系へ rename、互換 alias なし、責務移動なし。
- crate / directory:
  - `crates/llm-engine`, `crates/llm-engine-macros` が存在。
  - `crates/llm-worker`, `crates/llm-worker-macros` は存在しない。
- Cargo / Nix:
  - root `Cargo.toml` の members/default-members/workspace deps が `llm-engine` / `llm-engine-macros`。
  - `crates/pod/Cargo.toml` 等の依存も `llm-engine`。
  - `Cargo.lock` は旧 package 名から新 package 名へ置換済み。
  - `package.nix` の `cargoHash` 更新を確認。
- public API:
  - `crates/llm-engine/src/lib.rs` は `Engine`, `EngineConfig`, `EngineError`, `EngineResult`, `EngineRunOutput` を export。
  - `WorkerConfig` / `WorkerError` / `WorkerResult` / `WorkerState` / bare `RunOutput` は active tree で検出されず。
  - `llm-engine` / `llm-engine-macros` 内に `pub type Worker` 等の互換 alias は見つからず。
- stale reference:
  - `.yoi/tickets/**` と `docs/report/**` を除いた tracked active tree で `llm-worker`, `llm_worker`, `llm-worker-macros`, `llm_worker_macros` は検出されず。
- behavior / responsibility risk:
  - `crates/llm-worker/src/worker.rs` → `crates/llm-engine/src/engine.rs` は正規化比較で import order / formatting 程度の差分に収まっていることを確認。
  - `crates/llm-engine/README.md` に Pod names / sockets / process lifecycle / scope delegation / durable Pod state を持たない境界が明記されており、Runtime/Worker identity や socket/session authority の責務移動は見当たらない。
- whitespace:
  - `git diff --check 22598710..HEAD`: success。

未再実行:
- `cargo test -p llm-engine`
- `cargo test -p pod`
- `cargo check -p yoi`
- `nix build .#yoi --no-link`

上記は reviewer の read-only/no-write 条件を優先して再実行していない。coder 報告では成功済み。

Non-blocking note:
- `pod` 側には local variable や既存 runtime/manifest 文脈としての `worker` 語が残っているが、今回の acceptance 対象である old crate/import/API 名や `llm_engine::Worker` 互換 alias ではないため問題なし。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T13:57:16Z -->

## Implementation report

Integration outcome:

- Coder `yoi-coder-00001KVZD10ED-llm-engine` が implementation branch `work/00001KVZD10ED-llm-engine-rename` に実装を commit した。
  - implementation commit: `292fc4ea refactor: rename llm worker crate to engine`
- Reviewer `yoi-reviewer-00001KVZD10ED-llm-engine` は read-only review で `approve`。old crate directories removal、Cargo/Nix consistency、public Engine API、old active references なし、compatibility alias なし、behavior/responsibility movement なしを確認した。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` で branch `work/00001KVZD10ED-llm-engine-rename` を merge 済み。
  - merge commit: `254ecccb merge: 00001KVZD10ED llm engine rename`

Implemented behavior:
- `crates/llm-worker` -> `crates/llm-engine`
- `crates/llm-worker-macros` -> `crates/llm-engine-macros`
- Cargo package/dependency/import path を `llm-engine` / `llm_engine`, `llm-engine-macros` / `llm_engine_macros` に更新。
- Public turn-engine API を Engine 系へ rename。
  - `Worker` -> `Engine`
  - `WorkerConfig` -> `EngineConfig`
  - `WorkerError` -> `EngineError`
  - `WorkerResult` -> `EngineResult`
  - `WorkerState` -> `EngineState`
  - `RunOutput` -> `EngineRunOutput`
- `pod`, `manifest`, docs, examples, tests, Nix, `Cargo.lock`, `package.nix` hash を更新。
- old crate directories は削除済み。compatibility alias は追加していない。

Validation in Orchestrator worktree:
- `cargo test -p llm-engine`: success
- `cargo test -p pod`: success
- `cargo check -p yoi`: success
- active old reference grep excluding `.yoi/tickets/**` and `docs/report/**`: success, none found for:
  - `llm-worker`
  - `llm_worker`
  - `llm-worker-macros`
  - `llm_worker_macros`
- `crates/llm-engine` / `crates/llm-engine-macros` old public Worker API grep: success, none found for:
  - `\bWorker\b`
  - `WorkerConfig`
  - `WorkerError`
  - `WorkerResult`
  - `WorkerState`
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Notes:
- Historical `.yoi/tickets/**` and `docs/report/**` references to old names remain as durable history/report context and are not active API/docs guidance.
- Unlike recent Plugin full `cargo test -p pod` runs, this validation passed `cargo test -p pod` fully.

Next action:
- Mark Ticket done after this integration/validation evidence.
- Then stop related child Pods and remove only the child implementation worktree/branch.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T13:57:29Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Implementation is merged into the orchestration branch and validated.

Evidence:
- merge commit: `254ecccb merge: 00001KVZD10ED llm engine rename`
- reviewer result: approve
- validation in `/home/hare/Projects/yoi/.worktree/orchestration` succeeded:
  - `cargo test -p llm-engine`
  - `cargo test -p pod`
  - `cargo check -p yoi`
  - active old reference grep excluding `.yoi/tickets/**` and `docs/report/**`
  - old public Worker API grep in `crates/llm-engine` / `crates/llm-engine-macros`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Closure is not performed here; this state records implementation completion after merge/validation.

---
