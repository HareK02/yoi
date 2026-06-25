<!-- event: create author: "yoi ticket" at: 2026-06-23T16:34:39Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-23T18:26:48Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-23T18:26:48Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-23T19:25:09Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T19:26:52Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は Workspace backend の Worker runtime / Pod process launch / Ticket-Orchestration resolver の境界分離、typed spawn request/result、role-session 連携、安全境界、non-goals、validation を具体的に列挙している。
- `TicketRelationQuery` では blocking relation は 0 件、`TicketOrchestrationPlanQuery` では既存の ordering / blocker / conflict / accepted-plan 記録は 0 件だった。
- orchestration worktree は `## orchestration` で dirty changes なし、既存 implementation worktree は別 Ticket `00001KVSMJJNV-paused-ctrlx-cancel` と orchestration worktree のみで、この Ticket 用 worktree/branch はまだ無い。
- `crates/client/src/spawn.rs`、`crates/client/src/ticket_role.rs`、`crates/workspace-server/src/hosts.rs`、`crates/workspace-server/src/server.rs` を bounded に確認し、Ticket に書かれた現状認識と実装対象の code map が一致している。残る不確実性は local tactic / bounded investigation に収まる。

Evidence checked:
- Ticket body / thread: `item.md`、`thread.md`。thread は create、planning->ready、ready->queued のみで、未解決 blocker は記録されていない。
- related Ticket / orchestration plan records: relation 0 件、orchestration plan 0 件。
- code paths: `crates/client/src/spawn.rs`、`crates/client/src/ticket_role.rs`、`crates/workspace-server/src/hosts.rs`、`crates/workspace-server/src/server.rs`。
- workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` の `git status --short --branch` は clean な `## orchestration`。visible Pods は `yoi` peer と current `yoi-orchestrator` のみ。

IntentPacket:

Intent:
- Workspace backend から Worker spawn/stop/proxy を実装できる前段として、低レベル Pod process launch 境界と Worker runtime 境界、Ticket/Orchestration resolver 境界を分離し、既存 read-only `LocalRuntimeBridge` を操作境界へ拡張できる形に整理する。

Binding decisions / invariants:
- 低レベル Pod process launch layer は `yoi pod [POD_OPTIONS]` 起動だけを扱い、Ticket ID / Ticket role / Orchestration role を直接知らない。
- `SpawnConfig` をそのまま Workspace backend API 入力にしない。`ticket_role` は低レベル process config から分離する。
- Browser/API から `workspace_root` / `cwd` / executable path / raw profile selector を自由入力させない。backend/resolver が canonical workspace root、cwd、pod name、profile、initial input、workflow、launch policy、role-session claim を解決する。
- Orchestrator dedicated worktree 起動では runtime `workspace_root` と process `cwd` を混同しない。`workspace_root` は original/main workspace、`cwd` は orchestration worktree。
- Workspace backend が独自に `Command::new("yoi")` を組み立てて分岐する設計にしない。共有可能な低レベル launcher / config / acceptance evidence 境界を使う。
- この Ticket では Worker operation UI、SSE/WebSocket stream、full auth/permission、remote Host protocol、TypeScript protocol generation、Workspace identity `.yoi/workspace.toml`、StopPod/registry locking の追加修正は non-goal。

Requirements / acceptance criteria:
- Workspace backend runtime abstraction の責務境界を code/docs/tests で追える形にする。
- Pod process launch config と Ticket/Orchestration resolver 境界を分離し、低レベル config が Ticket/role/orchestration domain を持たないことを型・モジュール境界で示す。
- `LocalRuntimeBridge` 相当を Worker runtime trait/service として整理し、hosts/workers list、worker lookup、spawn/stop request、将来 proxy/stream 接続点を表現できるようにする。
- Worker spawn request/result の typed shape と acceptance evidence の扱いを導入する。
- role-session claim / Pod metadata / runtime registry の連携方針を実装境界または明文化された設計として残す。
- validation として少なくとも `cargo test -p yoi-workspace-server`、`cargo check -p yoi`、`cd web/workspace && deno task check && deno task build`、`git diff --check` を実施する。`nix build .#yoi --no-link` は変更量・依存/packaging 影響に応じて Orchestrator が最終判断する。

Implementation latitude:
- trait / struct 名は Ticket の例示に縛られず、既存 module organization に沿ってよい。
- `SpawnConfig` を rename/split するか、新規 `PodProcessLaunchConfig` / `PodLaunchOptions` を導入するかは coder が code map を見て選んでよい。
- API endpoint の完全実装ではなく、backend abstraction と typed request/result の初期 shape / handler 接続点を優先してよい。
- tests は既存 workspace-server/client test style に合わせて最小限でよい。

Escalate if:
- Browser から raw local path / raw executable / raw profile selector を受ける必要が出る。
- Ticket/Orchestration 情報を低レベル launcher に持たせないと実装できないと判断した場合。
- workspace_root / cwd / runtime identity / role-session authority の境界を変える必要が出る。
- public API endpoint shape をこの Ticket の初期案から大きく固定する必要が出る。
- existing Panel/role-session semantics を破壊する変更が必要になる。

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- 必要に応じて `nix build .#yoi --no-link`

Current code map:
- Primary: `crates/workspace-server/src/hosts.rs`, `crates/workspace-server/src/server.rs`, `crates/client/src/spawn.rs`, `crates/client/src/ticket_role.rs`。
- Secondary if needed: workspace API/client integration, role-session helpers, Pod runtime command utilities, docs/resources prompts only when implementation requires。
- Avoid: unrelated UI completion, broad permission model, remote Host protocol, root/original workspace operations。

Critical risks / reviewer focus:
- Ticket/role/orchestration fields leaking into the low-level launcher.
- Workspace runtime abstraction becoming a UI-specific concrete local bridge instead of a backend boundary.
- Browser/API accepting authority-bearing raw paths or executable/profile selectors.
- `workspace_root` and `cwd` confusion for dedicated Orchestrator/Coder/Reviewer launches.
- acceptance evidence treated as mere process start.
- role-session claim / metadata / runtime registry authority and hint semantics left ambiguous.

Next action:
- `queued -> inprogress` を記録してから、worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVTNAY20-worker-runtime-spawn` を作成し、multi-agent-workflow で sibling coder/reviewer loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T19:27:22Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、blocking relation は 0 件、accepted plan は `orch-plan-20260623-192711-1` のみだった。
- routing decision と IntentPacket は Ticket thread に記録済み。
- orchestration worktree は clean な `## orchestration` で、この Ticket 用 implementation worktree/branch はまだ作成していない。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVTNAY20-worker-runtime-spawn` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T19:30:10Z -->

## Implementation report

Implementation orchestration update:

- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVTNAY20-worker-runtime-spawn` と branch `work/00001KVTNAY20-worker-runtime-spawn` を作成した。
- child worktree では sparse-checkout を設定し、tracked `.yoi` project records は見える一方で `.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を除外する方針にした。
- Coder Pod `yoi-coder-00001KVTNAY20-worker-runtime` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。
- 初回 spawn は workspace root read scope が不足して失敗したため、implementation side effect は残さず scope を修正して再試行した。現在 reachable child Pod として起動済み。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T19:31:58Z -->

## Implementation report

Implementation orchestration recovery:

- `yoi-coder-00001KVTNAY20-worker-runtime` は provider API error `Unsupported content type` で初回実行・再送とも assistant output を生成できなかった。
- child worktree `/home/hare/Projects/yoi/.worktree/00001KVTNAY20-worker-runtime-spawn` は実装差分なしで、last commit は routing acceptance commit `5a8bcebd ticket: accept worker runtime spawn task` のままだった。
- 失敗 Pod は停止して scope を回収した。
- 実装 side effect の回復として、同じ child worktree/branch を使い、別 profile で Coder role を再起動する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T19:33:48Z -->

## Implementation report

Implementation orchestration blocked:

- Coder sibling execution is blocked by runtime/provider failures, not by Ticket requirements or repository state.
- `builtin:coder` Pod `yoi-coder-00001KVTNAY20-worker-runtime` started, but both initial run and retry failed before assistant output with provider API error: `Unsupported content type`.
- That Pod was stopped and scope reclaimed. child worktree status remained clean with no implementation diff.
- Recovery attempts using `inherit` (`yoi-coder-00001KVTNAY20-worker-runtime-2`), `builtin:default` (`...-3`), and `builtin:companion` (`...-4`) did not produce sockets within 10s.
- Worktree `/home/hare/Projects/yoi/.worktree/00001KVTNAY20-worker-runtime-spawn` and branch `work/00001KVTNAY20-worker-runtime-spawn` are prepared and clean at orchestration commit `b8388655`.
- `orch-plan-20260623-193340-2` に waiting-capacity note を残した。

Next action:
- Coder Pod/profile/provider startup issue の解消後に同じ worktree/branch で multi-agent workflow を再開する。
- あるいは人間が明示的に Orchestrator direct implementation を許可する場合のみ、Orchestrator がこの child worktree で実装へ進む。

---

<!-- event: implementation_report author: hare at: 2026-06-24T10:26:04Z -->

## Implementation report

# 実装報告: 00001KVTNAY20

## 変更概要

- `client::spawn` に `PodProcessLaunchConfig` と `PodProcessLaunchOptions` を導入し、低レベルの Pod プロセス起動設定から Ticket role marker を分離した。
- Ticket role 起動は `TicketRoleLaunchPlan::spawn_options()` 経由で hidden CLI marker を渡す形にし、`TicketRoleLaunchResult` に Run 受理証跡 (`TicketRoleLaunchAcceptanceEvidence`) を追加した。
- Workspace server の `LocalRuntimeBridge` を `WorkspaceWorkerRuntime` trait の実装として整理し、hosts/workers 一覧、worker lookup、spawn/stop typed request/result、将来の proxy/stream 接続点を型として追加した。
- Workspace 側の spawn request shape は policy intent ベースにし、browser/API caller から raw `workspace_root` / `cwd` / executable path / raw profile selector を受け取らない形にした。
- Dashboard/TUI 側の直接 spawn 呼び出しを新しい low-level config/options 分離に追従した。

## 変更ファイル

- `crates/client/src/lib.rs`
- `crates/client/src/spawn.rs`
- `crates/client/src/ticket_role.rs`
- `crates/tui/src/dashboard/mod.rs`
- `crates/tui/src/spawn.rs`
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`

## 検証結果

- `cargo test -p yoi-workspace-server`: 成功
- `cargo check -p yoi`: 成功
- `cd web/workspace && deno task check && deno task build`: 成功
- `cargo test -p client`: 成功（追加確認）
- `git diff --check`: 成功

## コミット

- 実装コミット: `217a4828d73ab553b5406cc7e22e43b1ec7be48e`

## 残リスク / 非ゴールとして残したもの

- `WorkspaceWorkerRuntime::spawn_worker` / `stop_worker` は typed boundary と request/result を用意した段階で、実際の Worker operation UI 完成、stream proxy、remote Host protocol、認可/権限、registry locking までは実装していない。
- low-level launcher は trusted in-process resolver からの追加 CLI args を受け取れるが、Ticket role などのドメイン概念は `PodProcessLaunchConfig` からは除外している。
- TS 型生成やフロントエンド API surface の追加は本 Ticket の非ゴールとして変更していない。


---

<!-- event: review author: yoi-reviewer-00001KVTNAY20-worker-runtime at: 2026-06-24T10:33:54Z status: approve -->

## Review: approve

Review result: approve

Read-only で Ticket `00001KVTNAY20`、implementation report、`a729d686..HEAD` の差分を確認した。ファイル変更・Ticket review 記録・merge/cleanup は reviewer では行っていない。

確認した主な点:
- 低レベル Pod launcher は `crates/client/src/spawn.rs` の `PodProcessLaunchConfig` / options に分離され、Ticket/role/orchestration 固有の型や field は入っていない。`--workspace` と process `cwd` も別 field のまま扱われている。
- Ticket role 起動は `crates/client/src/ticket_role.rs` 側に寄せられ、role/profile/workflow/prompt/初回 `Run` の組み立ては launcher ではなく role 層で処理されている。
- 起動成功は単なる process start ではなく、既存 socket readiness に加えて Ticket role launch では `Method::Run` 送信後の `UserMessage` / `InvokeStart(UserSend)` / `TurnStart` を待つ形になっている。
- Workspace server 側は `WorkspaceWorkerRuntime` trait と `WorkerSpawnRequest` / `WorkerStopRequest` / `WorkerProtocolProxyRequest` を導入し、現在の local 実装は Pod metadata read boundary + unsupported lifecycle operation として明示されている。Browser/API request shape に `workspace_root` / `cwd` / executable path / raw profile selector は受けていない。
- Dedicated Orchestrator launch では runtime `workspace_root` と process `cwd` を混同しないテストが追加されており、original workspace と orchestration checkout が分離されている。
- 非ゴールである実 scheduler、E2E、merge/cleanup automation 等の scope creep は入っていない。

Validation evidence:
- reviewer は read-only 制約のため build/test は再実行していない。
- 実装報告の検証結果として `cargo test -p yoi-workspace-server`、`cargo check -p yoi`、`cd web/workspace && deno task check && deno task build`、`cargo test -p client`、`git diff --check` の成功を確認した。
- reviewer 側では read-only な `git diff --check a729d686..HEAD` を実行し、問題なし。

Non-blocking notes:
- `WorkerSpawnRequest` はまだ実 endpoint 未接続かつ local runtime では `Unsupported` なのでこの Ticket では問題ない。今後実装時は `requested_worker_name` を backend 側で厳格に validate し、Browser 由来の値を Pod authority として扱わないことを明示すると安全。
- `launch_ticket_role_pod` の acceptance 待ちは fresh spawn 前提では妥当。将来既存 Pod restore/競合を扱う場合は、connect-time `Snapshot`/`Alert` の drain と「送信した Run に対応する evidence」の対応付けをより強くするとよい。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T10:34:55Z -->

## Implementation report

Integration outcome:

- Coder retry `yoi-coder-00001KVTNAY20-worker-runtime-retry1` が完走し、implementation branch `work/00001KVTNAY20-worker-runtime-spawn` に実装を commit した。
- Reviewer `yoi-reviewer-00001KVTNAY20-worker-runtime` は read-only review で `approve`。低レベル launcher と Ticket/role/orchestration resolver の分離、Workspace runtime abstraction、raw authority input 非受理、`workspace_root`/`cwd` 分離、acceptance evidence、scope creep なしを確認した。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` で branch `work/00001KVTNAY20-worker-runtime-spawn` を merge 済み。
  - merge commit: `97555bb5 merge: 00001KVTNAY20 worker runtime spawn`
  - implementation commit: `217a4828 feat: abstract worker runtime spawn boundary`
  - implementation report commit: `d62ab6e1 docs: record worker runtime implementation report`

Validation in Orchestrator worktree:
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `cd web/workspace && deno task check`: success
- `cd web/workspace && deno task build`: success
- `git diff --check`: success

Notes:
- Reviewer non-blocking note: future actual spawn endpoint should validate any requested worker name server-side and avoid treating Browser-provided values as Pod authority.
- Reviewer non-blocking note: future restore/competition handling may need stronger association between a sent `Run` and acceptance evidence after connect-time `Snapshot`/`Alert` drain.

Next action:
- Mark Ticket done after this integration/validation evidence.
- Then stop related child Pods and remove only the child implementation worktree/branch.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T10:35:01Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Implementation is merged into the orchestration branch and validated.

Evidence:
- merge commit: `97555bb5 merge: 00001KVTNAY20 worker runtime spawn`
- reviewer result: approve
- validation in `/home/hare/Projects/yoi/.worktree/orchestration` succeeded:
  - `cargo test -p yoi-workspace-server`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task build`
  - `git diff --check`

Closure is not performed here; this state records implementation completion after merge/validation.

---

<!-- event: state_changed author: hare at: 2026-06-25T14:13:52Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-25T14:13:52Z status: closed -->

## 完了

Implemented, reviewed, marked done, and merged into develop.


---
