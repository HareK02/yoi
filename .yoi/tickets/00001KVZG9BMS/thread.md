<!-- event: create author: "yoi ticket" at: 2026-06-25T13:42:37Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T14:08:22Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T14:08:22Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T14:13:35Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T14:14:52Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- outgoing `depends_on` は `00001KVZD10ED` で、`00001KVZD10ED` は done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependent `00001KVZBCQH4` は後続 worker-runtime creation であり、この Ticket の acceptance blocker ではない。
- Ticket body は `crates/pod` -> `crates/worker` crate/package/import rename、public execution-unit API rename、CLI/process surface update、non-goals、validation を具体的に列挙している。
- bounded context check で current active references to `crates/pod`, `pod::`, package `pod`, `yoi pod` process entrypoint, tests/docs/Nix を確認した。変更量は大きいが mechanical rename + bounded public surface alignment として進められる。
- CLI/process surface は risk だが、Ticket は backward compatibility alias 不要、dogfooding runtime/spawn path/scripts/tests 影響を明示処理することを要求している。設計未決定というより implementation slice 内の bounded choice と判断する。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。thread は create、planning->ready、ready->queued のみで未解決 blocker は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVZD10ED` is done; incoming dependent `00001KVZBCQH4`; routing 前 plan 0 件。accepted plan `orch-plan-20260625-141406-1` を記録済み。
- Code context: `git grep` で `crates/pod`, `pod::`, package/dependency `pod`, `yoi pod`, `Pod` public API refs を確認。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。queued Ticket はこの 1 件、inprogress Ticket は 0 件。

IntentPacket:

Intent:
- 現在の single execution-unit host crate `pod` を `worker` に rename し、実行単位としての `worker::Worker` と LLM turn engine `llm_engine::Engine` の命名境界を揃える。

Binding decisions / invariants:
- This is a rename / API terminology alignment Ticket, not a responsibility rewrite.
- `worker` crate remains the current single Worker host: input handling, llm-engine integration, event emission, session/transcript compatibility, tool registry, workflow integration, legacy socket compatibility.
- Do not implement `worker-runtime` crate or multi-worker Runtime API.
- Do not standalone-rename `pod-store` / `pod-registry` to `worker-store` / `worker-registry` in this Ticket.
- Do not change provider request/tool-call/history semantics.
- Do not create `pod` crate/import compatibility alias.
- Existing on-disk/socket/session compatibility may retain legacy `pod` strings only where explicitly legacy/internal and documented.

Requirements / acceptance criteria:
- `crates/worker` exists; `crates/pod` does not remain.
- Cargo package/import path are `worker`.
- Public execution-unit type is `worker::Worker`, not `pod::Pod`.
- Active repository references to `pod` crate / `pod::` import / `crates/pod` are gone except intentional legacy context.
- Dependent crates compile against `worker` crate.
- `llm_engine::Engine` vs `worker::Worker` boundary is clear in code/docs/comments.
- Existing process/socket/session compatibility path still works or is explicitly updated without old-name alias.
- `pod-store` / `pod-registry` are not renamed as standalone crates.
- Validation target includes `cargo test -p worker`, `cargo test -p yoi` or relevant CLI tests, `cargo check -p yoi`, `git diff --check`, `nix build .#yoi --no-link`.

Implementation latitude:
- Exact CLI command spelling may be updated according to Ticket requirement, but no backward alias should be added unless a hard blocker appears. If command migration threatens current runtime dogfooding assumptions, escalate.
- Internal legacy file/socket/session names may remain only when required for compatibility and must be clearly legacy/internal, not active API guidance.
- Type/module rename can be staged mechanically; prioritize compile/test correctness and grep evidence.

Escalate if:
- Rename requires broad runtime architecture rewrite or worker-runtime implementation.
- Current process launch/spawn mechanics cannot work without a compatibility `pod` command/alias.
- `pod-store` / `pod-registry` must be renamed for compile correctness.
- Session/socket/on-disk migration would be required beyond explicit legacy compatibility labels.
- Behavior changes unrelated to naming are needed.

Validation:
- `cargo test -p worker`
- `cargo test -p yoi` or focused process/CLI tests
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- grep evidence for stale active references.

Current code map:
- Primary: `crates/pod`, workspace `Cargo.toml`, dependent crates (`crates/yoi`, `crates/tui`, workspace-server/client as discovered), tests/docs/resources, `Cargo.lock`, `package.nix`.
- Avoid: `pod-store` / `pod-registry` standalone rename, worker-runtime implementation, root/original workspace operations.

Critical risks / reviewer focus:
- stale active `pod` crate/import/directory references.
- hidden compatibility alias.
- breaking runtime process launch / spawn command path.
- accidentally renaming persistence crates out of scope.
- behavior changes mixed into mechanical rename.
- confusion between `llm_engine::Engine` and `worker::Worker` responsibilities.

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVZG9BMS-worker-crate-rename` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T14:15:18Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependency `00001KVZD10ED` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260625-141406-1` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVZG9BMS-worker-crate-rename` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T14:16:23Z -->

## Implementation report

Implementation orchestration update:

- prerequisite `00001KVZD10ED` が done であることを確認し、`00001KVZG9BMS` を受理した。
- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVZG9BMS-worker-crate-rename` と branch `work/00001KVZG9BMS-worker-crate-rename` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVZG9BMS-worker-rename` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVZG9BMS-worker-rename at: 2026-06-25T15:07:12Z -->

## Implementation report

Implementation report:

実装概要:
- `crates/pod` を `crates/worker` へ改名。
  - Cargo package/import path を `pod` から `worker` へ更新。
  - `worker::Worker`, `WorkerConfig`, `WorkerState`, `WorkerEvent`, `WorkerClient`, `WorkerRuntimeCommand` など Worker 用語へ更新。
- CLI/process surface を `yoi pod` から `yoi worker` へ変更。
  - 後方互換の `yoi pod` alias は追加していません。
- `llm_engine::Engine` と `worker::Worker` の境界を明確化。
  - 既存 manifest の turn-engine 側設定は `engine` 用語へ寄せ、Worker host 側設定と分離しました。
- `pod-store` / `pod-registry` は crate/package としては改名していません。
  - ただし Worker API と整合するよう、関連する型・呼び出し側は Worker 用語へ更新。
  - 既存互換・内部文脈として、legacy on-disk metadata directory `pods` などは残しています。
- resources/prompts、profiles、docs、Nix、Cargo workspace、Cargo.lock を更新。
- `package.nix` の `cargoHash` を更新。

主な変更ファイル:
- `crates/pod/**` → `crates/worker/**`
- `crates/client/src/pod_client.rs` → `crates/client/src/worker_client.rs`
- `crates/tui/src/pod_list.rs` → `crates/tui/src/worker_list.rs`
- `crates/yoi/src/pod_cleanup_cli.rs` → `crates/yoi/src/worker_cleanup_cli.rs`
- `resources/prompts/pod/**` → `resources/prompts/worker/**`
- `resources/prompts/common/pod-orchestration.md` → `resources/prompts/common/worker-orchestration.md`
- `docs/design/pod-session-state.md` → `docs/design/worker-session-state.md`
- `Cargo.toml`, `Cargo.lock`, `package.nix`
- dependent crates: `protocol`, `manifest`, `tui`, `client`, `workspace-server`, `yoi`, docs/resources

Validation:
- `cargo test -p worker`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Grep evidence:
- `crates/pod` refs: intentional `pod-store` / `pod-registry` path を除きなし
- `pod::` refs: なし
- package/import `pod` refs: intentional `pod-store` / `pod-registry` dependencies を除きなし
- old public `Pod*` execution-unit type refs: intentional exclusions を除きなし
- active CLI refs:
  - `yoi pod`: なし
  - `"pod" =>`: なし
  - `--pod`: なし
  - `POD_OPTIONS`: なし
  - `parse_pod`: なし
  - `pod_name`: intentional exclusions を除きなし

残っている lower-case `pod` は主に以下の intentional legacy/internal context:
- crate/package name として残す指定のある `pod-store`
- crate/package name として残す指定のある `pod-registry`
- legacy on-disk metadata directory `pods`
- related comments/tests describing those legacy/internal boundaries

Commit:
- `6c59fe927b8094f735651ef0e18fad5e8b62ea83 refactor: rename pod crate to worker`

Residual risks / notes:
- `pod-store` / `pod-registry` crate 名は Ticket invariant に従って残しています。そのため grep 上の lower-case `pod` は完全にはゼロになりません。
- legacy on-disk directory `pods` は既存 metadata/session compatibility のため維持しました。
- merge / close / cleanup は実施していません。

---
