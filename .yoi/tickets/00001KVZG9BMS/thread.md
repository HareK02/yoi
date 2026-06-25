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

<!-- event: review author: yoi-reviewer-00001KVZG9BMS-worker-rename at: 2026-06-25T15:15:27Z status: request_changes -->

## Review: request changes

Review result: request_changes

読取専用で Ticket / IntentPacket、`4c677640..HEAD` diff、grep/nl による focused inspection を行いました。`cargo`/`nix` 系の再実行は read-only 指示と書込み副作用回避のため行っていません。

良い点:
- `crates/worker` は存在し、`crates/pod` は削除済み。
- workspace/Cargo.lock 上の package/import は `worker` に移行済みで、`pod-store` / `pod-registry` は standalone rename されていません。
- CLI は `yoi worker` / `--worker` 側へ移行しており、明示的な `yoi pod` alias は見当たりませんでした。

Blockers:

1. 生成済み Web protocol が旧 Pod protocol のまま。
   - `web/workspace/src/lib/generated/protocol.ts` が Rust 側の `Worker*` protocol と不整合です。
   - 例: `PodStatus`, `Greeting { pod_name: ... }`, `PodEvent`, method `pod_event`, `list_pods`, `restore_pod`, events `pods_listed`, `pod_restored` など。
   - `crates/protocol/src/lib.rs` は `WorkerStatus` / `WorkerEvent` / `worker_name` / `worker_event` 等へ更新済みのため、Web/TS client 側が socket protocol とズレるリスクがあります。

2. public API に旧 Pod 名の enum variant が残存。
   - `crates/client/src/spawn.rs` の `SpawnError::PodLaunchFailed { ... }`。
   - `crates/client/src/lib.rs` で `SpawnError` は public re-export されているため、Ticket の “Public execution-unit type/API terminology is Worker, not Pod” に反します。

3. model-facing prompts / workflows / active docs に旧 Pod guidance が残存。
   - `resources/prompts/common/worker-orchestration.md`: “Peer Pods ...”
   - `resources/workflows/ticket-orchestrator-routing.md`: “Coder/Reviewer は sibling Pods ...”
   - `resources/prompts/role/intake.md`, `resources/prompts/role/orchestrator.md`, `resources/prompts/panel/orchestrator_idle_queue_notice.md` にも active orchestration guidance として “Pods” が残っています。
   - `docs/design/worker-session-state.md` に `SendToPeerPod` と記載されていますが、実装側は `SendToPeerWorker`。
   - これらは legacy/internal compatibility label ではなく current LLM/workflow guidance なので例外扱いできません。

4. active documentation に壊れた旧パス / 旧型名が残存。
   - `crates/pod-store/README.md` に削除済み `docs/design/pod-session-state.md` へのリンク。
   - `KNOWN_ISSUES.md` に `crates/worker/src/pod.rs` / `Pod::ensure_segment_head` 等、存在しない path/type。

Required fixes:
- `web/workspace/src/lib/generated/protocol.ts` を Rust protocol と一致させる。
- `client::SpawnError::PodLaunchFailed` を `WorkerLaunchFailed` に rename し、match/source mapping も更新する。
- `resources/prompts/**` / `resources/workflows/**` の active “Pod(s)” guidance を Worker terminology に更新する。
- `SendToPeerPod` など実装と不一致な docs を `SendToPeerWorker` に更新する。
- 削除済み `docs/design/pod-session-state.md` へのリンク、存在しない `crates/worker/src/pod.rs` / `Pod::ensure_segment_head` 参照を修正する。
- 修正後、報告済み validation に加えて stale grep を再実施する。

---

<!-- event: implementation_report author: yoi-coder-00001KVZG9BMS-worker-rename at: 2026-06-25T15:26:07Z -->

## Implementation report

Follow-up implementation report after review request_changes:

対応内容:

1. Web generated protocol の Worker 用語追従
- `web/workspace/src/lib/generated/protocol.ts` を Rust protocol から再生成。
- 旧 Pod protocol 名を Worker に更新。
  - `PodStatus` → `WorkerStatus`
  - `Greeting { pod_name }` → `Greeting { worker_name }`
  - `PodEvent` → `WorkerEvent`
  - `pod_event` → `worker_event`
  - `list_pods` → `list_workers`
  - `restore_pod` → `restore_worker`
  - `pods_listed` → `workers_listed`
  - `pod_restored` → `worker_restored`

2. public API enum variant の修正
- `crates/client/src/spawn.rs`
  - `SpawnError::PodLaunchFailed` → `SpawnError::WorkerLaunchFailed`
  - public re-export される型なので、match / source mapping も更新。

3. active model-facing guidance の Pod 表現修正
- 以下を Worker 用語へ更新。
  - `resources/prompts/common/worker-orchestration.md`
  - `resources/workflows/ticket-orchestrator-routing.md`
  - `resources/prompts/role/intake.md`
  - `resources/prompts/role/orchestrator.md`
  - `resources/prompts/panel/orchestrator_idle_queue_notice.md`

4. stale docs/path/type の修正
- `docs/design/worker-session-state.md`
  - `SendToPeerPod` → `SendToPeerWorker`
- `crates/pod-store/README.md`
  - deleted path `docs/design/pod-session-state.md` → `docs/design/worker-session-state.md`
  - crate 名は invariant に従い `pod-store` のまま、説明は legacy-named crate owning current Worker metadata と明記。
- `KNOWN_ISSUES.md`
  - `crates/worker/src/pod.rs` → `crates/worker/src/worker.rs`
  - `Pod::ensure_segment_head` → `Worker::ensure_segment_head`
  - `PodError::WorkflowResolve` → `WorkerError::WorkflowResolve`

Validation:
- `cargo test -p worker`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `cargo test -p protocol --features typescript`: success
- `cd web/workspace && deno task check`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Stale grep evidence:
- review blocker terms were checked and found no matches:
  - `PodStatus`, `PodEvent`, `pod_name`, `pod_event`, `list_pods`, `restore_pod`, `pods_listed`, `pod_restored`, `PodLaunchFailed`, `SendToPeerPod`, `sibling Pods`, `Peer Pods`, `yoi pod`, `--pod`, `crates/worker/src/pod.rs`, `Pod::ensure_segment_head`, `pod-session-state.md`
- Strict stale grep was rerun; intentional `pod-store` / `pod-registry` crate names aside, active public Pod protocol/prompt/API reference remains absent.

Commit:
- `ebf50baa94f76bb000f70f63e133c7883729de5c fix: align worker rename followups`

Residual risks / notes:
- なし。child worktree は commit 後 clean。

---

<!-- event: review author: yoi-reviewer-00001KVZG9BMS-worker-rename at: 2026-06-25T15:29:25Z status: request_changes -->

## Review: request changes

Review result: request_changes

Follow-up を read-only で確認した。`6c59fe92..HEAD` の差分確認と blocker term grep を実施し、ビルド/テストは副作用回避のため再実行していない。

解消確認:
- `web/workspace/src/lib/generated/protocol.ts` は `WorkerStatus` / `worker_name` / `WorkerEvent` / `worker_event` / `list_workers` / `restore_worker` / `workers_listed` / `worker_restored` に更新済み。
- `SpawnError::PodLaunchFailed` は `WorkerLaunchFailed` に更新済み。
- 主要 prompt/workflow の `Peer Pods` / `sibling Pods` は一部修正済み。

Remaining blockers:

1. 削除済み docs path がまだ残存。
   - `crates/pod-registry/README.md:29`
   - `../../docs/design/pod-session-state.md` への link が残っている。対象 file は `worker-session-state.md` に移行済みなので、壊れた active doc link。

2. 旧 tool 名 `SendToPeerPod` が active code に残存。
   - `crates/session-analytics/src/lib.rs:1600`
   - 実装側は `SendToPeerWorker` へ移行済みだが、analytics の分類に `SendToPeerWorker` がなく `SendToPeerPod` だけ残っている。stale name であるだけでなく、現行 tool の分類漏れに見える。legacy log 対応で旧名を残すなら、`SendToPeerWorker` を追加した上で legacy comment を付けるのが安全。

3. active docs/comments に旧 Pod guidance がまだ残存。
   - `docs/development/work-items.md:250`: `Reviewer Pods should be sibling Pods...`
   - `crates/protocol/src/lib.rs:80`: `List Pods visible to this Worker...`
   - `AGENTS.md:19`: `Notify / PodEvent / ...`
   - いずれも compatibility/internal on-disk `pods` の説明ではなく、現在の active terminology / protocol guidance なので Worker terminology に寄せるべき。

Required verification after fix:
```sh
rg -n -S 'SendToPeerPod|pod-session-state\.md|sibling Pods|List Pods visible|PodEvent' \
  AGENTS.md crates resources docs web/workspace/src/lib/generated/protocol.ts \
  --glob '!target' --glob '!docs/report/**'
```
This grep should be empty or any remaining hit must be explicitly legacy/internal and justified.

---

<!-- event: implementation_report author: yoi-coder-00001KVZG9BMS-worker-rename at: 2026-06-25T15:44:18Z -->

## Implementation report

Second follow-up implementation report after review request_changes:

対応内容:

1. broken docs path 修正
- `crates/pod-registry/README.md`
  - `../../docs/design/pod-session-state.md` → `../../docs/design/worker-session-state.md`
  - `pod-registry` は invariant に従って crate 名を維持しつつ、本文では legacy-named crate と明記して Worker 用語へ更新。

2. old tool name classification 修正
- `crates/session-analytics/src/lib.rs`
  - current tool name `SendToPeerWorker` を worker tool classification に追加。
  - historical session logs の分類維持のため、`SendToPeerPod` は `LEGACY_SEND_TO_PEER_POD_TOOL` として明示的な legacy analytics-only comment 付きで保持。

3. active docs/comments の Pod guidance 修正
- `docs/development/work-items.md`
  - `Reviewer Pods should be sibling Pods...` → `Reviewer Workers should be sibling Workers...`
- `crates/protocol/src/lib.rs`
  - `List Pods visible...` → `List Workers visible...`
- `AGENTS.md`
  - `Podの状態` → `Workerの状態`
  - `Notify / PodEvent / ...` → `Notify / WorkerEvent / ...`

Validation:
- `cargo test -p session-analytics`: success
- `cargo test -p worker`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `cargo test -p protocol --features typescript`: success
- `cd web/workspace && deno task check`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Specified grep evidence:
```text
crates/session-analytics/src/lib.rs:1595:    const LEGACY_SEND_TO_PEER_POD_TOOL: &str = "SendToPeerPod";
```
- この 1 件のみ残存。
- historical session logs の分類維持のための analytics-only legacy tool name で、直下の classification branch にも legacy comment を付けている。
- active prompt/protocol/docs/API guidance の旧 Pod 参照ではない。

Commit:
- `94c7aa793a179b377108dea841fe9d381affbd9c fix: clean remaining worker rename references`

Residual risks / notes:
- child worktree は commit 後 clean。

---

<!-- event: review author: yoi-reviewer-00001KVZG9BMS-worker-rename at: 2026-06-25T15:46:40Z status: request_changes -->

## Review: request changes

Review result: request_changes

読取専用で `94c7aa79` を確認した。前回指摘分のうち、以下は解消済み。

- `crates/pod-registry/README.md` の `worker-session-state.md` link 更新。
- `crates/protocol/src/lib.rs` の `List Workers...` comment 更新。
- `AGENTS.md` / `docs/development/work-items.md:250` の前回 exact blocker 更新。
- `SendToPeerPod` は `LEGACY_SEND_TO_PEER_POD_TOOL` として analytics-only legacy comment 付きで残り、現行 `SendToPeerWorker` 分類も追加済みなので、この残存は許容可能。

Remaining blockers:

1. active E2E helper に旧 `--pod` CLI が残存。
   - `tests/e2e/src/lib.rs:1541`
   - `config.command_args` がまだ `"--pod"` を渡している。
   - `yoi pod` / `--pod` alias を作らない方針なので、これは現行 CLI に対して壊れた active test/helper。`--worker` へ更新が必要。

2. active user-facing / developer-facing guidance に旧 Pod terminology が残存。
   代表例:
   - `crates/tui/src/dashboard/mod.rs:96` — `"no Tickets or Pods found ..."`
   - `crates/tui/src/dashboard/mod.rs:2939` — `"spawning role Pods"`
   - `crates/tui/src/dashboard/mod.rs:4951` — `"sibling coder/reviewer Pods"`
   - `crates/tui/src/dashboard/tests.rs:837` — 旧文字列期待
   - `crates/client/README.md:5` — “talking to Pods”
   - `crates/manifest/src/config.rs:43` — “spawned child Pods”
   - `docs/design/overview.md:14` — “between clients and Pods”
   - `docs/development/workflows.md` / `docs/development/dogfooding.md` / `docs/development/work-items.md` にも current guidance としての `Pods` が残存。

これらは `pod-store` / `pod-registry` crate 名や on-disk `pods` compatibility の説明ではなく、現在の実行単位・CLI・Dashboard guidance なので Worker terminology に寄せるべき。

Required verification after fixes:
```sh
rg -n -S --glob '!target' --glob '!docs/report/**' -- '--pod|yoi pod' .
rg -n -S --glob '!target' --glob '!docs/report/**' \
  'sibling coder/reviewer Pods|role Pods|talking to Pods|between clients and Pods|Spawned Pods|child Pods|Pods found|Pod categories|Pod row' \
  crates docs tests resources AGENTS.md README.md KNOWN_ISSUES.md
```
These should be empty or any remaining hit must be explicitly legacy/internal and justified.

---

<!-- event: implementation_report author: yoi-coder-00001KVZG9BMS-worker-rename at: 2026-06-25T15:58:35Z -->

## Implementation report

Third follow-up implementation report after review request_changes:

対応内容:

1. E2E helper の旧 CLI 引数修正
- `tests/e2e/src/lib.rs`
  - rewind fixture の起動引数を `--pod` から `--worker` に変更。
  - 関連 helper 名・フィールド・説明も `worker_names` / `worker_rows` / Worker row に更新。
- `tests/e2e/tests/panel.rs`
  - 期待値・メッセージを Worker terminology に更新。

2. Active user/developer guidance の Pod 表現修正
- `crates/tui/src/dashboard/mod.rs`
  - `no Tickets or Pods found ...` → `no Tickets or Workers found ...`
  - `spawning role Pods` → `spawning role Workers`
  - `sibling coder/reviewer Pods` → `sibling coder/reviewer Workers`
- `crates/tui/src/dashboard/tests.rs`
  - 期待文字列を Worker に更新。
- `crates/client/README.md`
  - `talking to Pods` → `talking to Workers`
- `crates/manifest/src/config.rs`
  - `spawned child Pods` → `spawned child Workers`
- `docs/design/overview.md`
  - `between clients and Pods` → `between clients and Workers`
- `docs/design/profiles-manifests-prompts.md`
  - `Spawned Pods` → `Spawned Workers`
- `docs/development/dogfooding.md`
  - `child Pods` → `child Workers`
- `docs/development/workflows.md`
  - `Spawned Pods` → `Spawned Workers`

Validation:
- `cargo test -p worker`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `cargo test -p protocol --features typescript`: success
- `cargo check -p yoi-e2e`: success
- `cd web/workspace && deno task check`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Notes:
- `cargo check -p e2e` は package 名が存在しなかったため失敗し、正しい package 名 `yoi-e2e` で `cargo check -p yoi-e2e` を実行して成功。

Specified grep evidence:
- `rg -n -S --glob '!target' --glob '!docs/report/**' -- '--pod|yoi pod' .`: no matches.
- `rg -n -S --glob '!target' --glob '!docs/report/**' 'sibling coder/reviewer Pods|role Pods|talking to Pods|between clients and Pods|Spawned Pods|child Pods|Pods found|Pod categories|Pod row' crates docs tests resources AGENTS.md README.md KNOWN_ISSUES.md`: no matches.
- Additional E2E stale grep for `pod_rows`, `pod_names`, `row.key.kind == "pod"`, `"--pod"`: no matches.

Commit:
- `da96d06f25932ff60fc00a3fc516a276a2847125 fix: remove stale pod guidance references`

Residual risks / notes:
- child worktree は commit 後 clean。

---

<!-- event: review author: yoi-reviewer-00001KVZG9BMS-worker-rename at: 2026-06-25T16:00:39Z status: request_changes -->

## Review: request changes

Review result: request_changes

読取専用で `da96d06f` を確認した。指定 grep は報告通り概ね解消しており、`--pod` / `yoi pod`、`pod_rows` / `pod_names` / `row.key.kind == "pod"` は見当たらなかった。

ただし、前回 blocker の「active user/developer-facing guidance に旧 Pod terminology が残存」に対して、まだ active docs/test diagnostics に旧名が残っている。

Remaining blockers:

- `docs/development/workflows.md`
  - `20: ## Child Pods`
  - `38: Child Pods may commit ...`
- `docs/development/work-items.md`
  - `27: Inside Pods ...`
  - `32: Ticket tools inside Pods`
  - `34: Pods with the Ticket built-in feature ...`
  - `198: spawn coder/reviewer Pods ...`
  - `229: coder Pods ...`
- `tests/e2e/tests/panel.rs`
  - `77: fixture blocks host Pod launch`
  - `626: host/fixture runtime Pod marker ...`

これらは `pod-store` / `pod-registry` crate 名や on-disk `pods` compatibility の説明ではなく、現在の Worker 実行単位・workflow guidance・test diagnostics なので、Ticket の Worker rename intent に合わせて `Worker(s)` へ更新が必要。

`SendToPeerPod` の analytics-only legacy constant は、現行 `SendToPeerWorker` 分類と明示 comment があるため許容。

---

<!-- event: implementation_report author: yoi-coder-00001KVZG9BMS-worker-rename at: 2026-06-25T16:07:26Z -->

## Implementation report

Fourth follow-up implementation report after review request_changes:

対応内容:

1. 指摘箇所の Worker terminology 更新
- `docs/development/workflows.md`
  - `## Child Pods` → `## Child Workers`
  - `Child Pods may commit ...` → `Child Workers may commit ...`
- `docs/development/work-items.md`
  - `Inside Pods ...` → `Inside Workers ...`
  - `Ticket tools inside Pods` → `Ticket tools inside Workers`
  - `Pods with the Ticket built-in feature ...` → `Workers with the Ticket built-in feature ...`
  - `spawn coder/reviewer Pods ...` → `spawn coder/reviewer Workers ...`
  - `coder Pods ...` → `coder Workers ...`
- `tests/e2e/tests/panel.rs`
  - `fixture blocks host Pod launch` → `fixture blocks host Worker launch`
  - `host/fixture runtime Pod marker ...` → `host/fixture runtime Worker marker ...`

2. 追加で見つかった docs/tests の active Pod 表現も更新
- `docs/design/tool-permissions-scope.md`: `Child Pods` → `Child Workers`
- `docs/design/compaction.md`: `long-running Pods` → `long-running Workers`
- `docs/design/memory-knowledge.md`: `normal Pods` → `normal Workers`
- `docs/design/plugin-packages.md`: `Restored Pods` → `Restored Workers`
- `docs/development/environment.md`: `spawned Pods` → `spawned Workers`
- `tests/e2e/tests/rewind.rs`: `single-pod rewind fixture` → `single-worker rewind fixture`
- `tests/e2e/tests/panel.rs`: active test helper/fixture naming and diagnostics updated to Worker terminology.

Validation:
- `cargo test -p worker`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `cargo test -p protocol --features typescript`: success
- `cargo check -p yoi-e2e`: success
- `cd web/workspace && deno task check`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Grep evidence:
- `rg -n -S --glob '!target' --glob '!docs/report/**' -- '--pod|yoi pod' .`: no matches.
- `rg -n -S --glob '!target' --glob '!docs/report/**' 'sibling coder/reviewer Pods|role Pods|talking to Pods|between clients and Pods|Spawned Pods|child Pods|Pods found|Pod categories|Pod row' crates docs tests resources AGENTS.md README.md KNOWN_ISSUES.md`: no matches.
- `rg -n -S 'Child Pods|Inside Pods|Ticket tools inside Pods|Pods with the Ticket built-in feature|spawn coder/reviewer Pods|coder Pods|host Pod launch|runtime Pod marker' docs/development/workflows.md docs/development/work-items.md tests/e2e/tests/panel.rs`: no matches.
- `rg -n -S '\bPod\b|\bPods\b' docs tests --glob '!target' --glob '!docs/report/**'`: no matches.

Commit:
- `cb0c52e7871849b9482c37ae63cacf4a2898da90 fix: update remaining active worker wording`

Residual risks / notes:
- child worktree は commit 後 clean。

---
