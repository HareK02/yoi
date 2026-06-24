<!-- event: create author: "yoi ticket" at: 2026-06-24T09:11:38Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T09:21:22Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T09:21:22Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T09:22:55Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T09:24:52Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は Worker runtime registry / overview reporting / runtime capability / identity / visibility / raw-session boundary / local Pod runtime migration を具体的に列挙しており、実装意図と受け入れ条件は implementation-ready に近い。
- `TicketRelationQuery` では blocking relation は 0 件、初回 `TicketOrchestrationPlanQuery` では既存 plan record は 0 件だった。
- ただし現在 `00001KVTNAY20` (`Abstract Workspace Worker runtime spawn operations`) が `inprogress` で、同じ `LocalRuntimeBridge` / Workspace Worker runtime abstraction / runtime identity・cwd 境界と `crates/workspace-server/src/hosts.rs` 周辺を扱っている。
- `00001KVTNAY20` は multi-agent startup が provider/runtime error で停止しており、implementation worktree は clean だが Coder sibling capacity が実質利用できない状態である。
- この Ticket を今 `queued -> inprogress` にすると、同じ runtime abstraction の registry 側と spawn 側を別 branch で同時に固定して conflict risk が高い。さらに Coder/Reviewer loop を起動できないため、acceptance 後に進められない。

Evidence checked:
- Ticket body / thread: `00001KVWECEQG` の item/thread。thread は create、planning->ready、ready->queued のみで未解決 planning question は記録されていない。
- Relations / orchestration plan: `00001KVWECEQG` は relation 0 件、既存 plan 0 件。routing 中に `orch-plan-20260624-092429-1` (`do_not_parallelize` with `00001KVTNAY20`) と `orch-plan-20260624-092435-2` (waiting capacity note) を記録した。
- Related Ticket: `00001KVTNAY20` は `inprogress`、accepted plan と waiting-capacity note があり、Coder Pod startup/provider failure により実装未開始。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。既存 implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVTNAY20-worker-runtime-spawn` は clean at `a729d686`。
- Visible Pods: `yoi`, current `yoi-orchestrator`, and stopped/restorable failed `yoi-coder-00001KVTNAY20-worker-runtime` metadata only; active usable Coder/Reviewer sibling は無い。

Decision:
- `00001KVWECEQG` は planning へ戻さない。Ticket の missing decision / missing information は現時点で具体化していない。
- ただし `queued -> inprogress` acceptance は行わず、queued のまま待機させる。
- Blocker は Ticket 要件ではなく、(1) `00001KVTNAY20` との runtime abstraction/code-surface conflict、(2) Coder sibling runtime/provider capacity failure。

Next action:
- 先に `00001KVTNAY20` の Coder runtime/provider blocker を解消して multi-agent workflow を再開する、または人間が Orchestrator direct implementation / ordering を明示的に許可する。
- その後、この Ticket を再 routing し、必要なら `00001KVTNAY20` の実装結果に合わせて registry abstraction の IntentPacket を作る。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T10:37:25Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーから「進めて」と明示 follow-up があり、以前 `queued` のまま待機させた blocker を再確認した。
- Ticket body は Worker runtime registry / overview reporting / runtime capability / identity / visibility / raw-session boundary / local Pod runtime migration を具体的に列挙しており、observable な acceptance criteria と reviewer focus がある。
- `TicketRelationQuery` は blocking relation 0 件。
- 以前の `do_not_parallelize` / waiting-capacity note は `00001KVTNAY20` が `done` になり、merge/validation 済みになったことで解消済みとして扱える。
- 現在の `crates/workspace-server/src/hosts.rs` には `00001KVTNAY20` の結果として `WorkspaceWorkerRuntime`、typed spawn/stop/proxy request/result、local Pod metadata overview が入り、今回の registry/capability/overview abstraction をその上に重ねられる。残る不確実性は local tactic / bounded investigation に収まる。
- orchestration worktree は clean で、同一 Ticket 用 worktree/branch はまだ無い。

Evidence checked:
- Ticket body / thread: `00001KVWECEQG` の item/thread。前回 routing decision 以降の missing planning question はなし。
- Relations / orchestration plan: relation 0 件。既存 plan は前回の `do_not_parallelize` / waiting-capacity と、今回記録した accepted plan `orch-plan-20260624-103655-3`。
- Related Ticket: `00001KVTNAY20` は `done`。review approve、orchestration merge、validation success、child cleanup 済み。
- Code map: `crates/workspace-server/src/hosts.rs`, `crates/workspace-server/src/server.rs` を current orchestration branch で確認。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。

IntentPacket:

Intent:
- Workspace backend の Worker runtime surface を、single local bridge から registry / host / capability / overview reporting の境界へ抽象化し、Dashboard/API が runtime 一覧・worker 一覧・capability/diagnostic を安全に表示できる基盤を作る。

Binding decisions / invariants:
- registry/overview は local Pod metadata と role-session/runtime metadata の安全な summary を扱う。raw session contents や unchecked private/runtime paths を API response に出さない。
- Browser/API 由来の値を runtime authority として扱わない。worker/host id は backend が解決・検証した bounded identifier として扱う。
- runtime identity / workspace_root / process cwd / role-session claim は混同しない。
- `00001KVTNAY20` で入った low-level launch boundary と Ticket/role/orchestration resolver separation を崩さない。
- remote Host protocol、full scheduler、operation UI、stream/proxy 実装、permission/auth の完成、raw session inspection は non-goal。

Requirements / acceptance criteria:
- Worker runtime registry/service abstraction が、複数 runtime/host summary と capability/diagnostic reporting を表現できる。
- `LocalRuntimeBridge` 由来の local runtime が registry の一実装/entry として扱われ、API handler が concrete bridge に直接密結合しすぎない。
- overview response は bounded/safe fields のみで、local Pod metadata failure や unavailable capability を diagnostic として返せる。
- existing `/api/hosts` / `/api/workers` semantics を維持または明確に migration し、UI が使う shape を壊す場合は対応も含める。
- validation として少なくとも `cargo test -p yoi-workspace-server`, `cargo check -p yoi`, `cd web/workspace && deno task check && deno task build`, `git diff --check` を実施する。

Implementation latitude:
- registry trait/struct 名、module split、capability enum/string shape、diagnostic aggregation の詳細は coder が既存 code style に合わせて決めてよい。
- API の大規模追加ではなく、current endpoints の内部境界整理と typed overview/capability shape の初期導入を優先してよい。
- UI 変更は必要最小限でよい。既存 web build/typecheck を通すための追随は行ってよい。

Escalate if:
- Browser/API から raw path/session/executable/profile 等を受けないと実装できない。
- runtime registry を scheduler/lease/remote Host protocol として固定する必要が出る。
- `00001KVTNAY20` の launch boundary を戻す/破る必要が出る。
- raw session content や private metadata を overview に出す必要がある。

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- 影響範囲に応じて追加 Rust tests。

Current code map:
- Primary: `crates/workspace-server/src/hosts.rs`, `crates/workspace-server/src/server.rs`。
- Secondary: Workspace web API consumers under `web/workspace`, role-session/local metadata helpers if needed, `crates/client/src/spawn.rs` / `crates/client/src/ticket_role.rs` only to preserve boundaries.
- Avoid: root/original workspace operations, unrelated Panel UI completion, remote Host protocol, scheduler/queue implementation, raw session reading.

Critical risks / reviewer focus:
- registry abstraction becoming a scheduler or authority layer by accident.
- leaking raw session/private paths/unchecked metadata into overview responses.
- host/worker ids derived from user-controlled strings without bounding/validation.
- breaking the launch boundary from `00001KVTNAY20`.
- conflating runtime workspace root, process cwd, role-session claim, and local metadata hints.

Next action:
- `queued -> inprogress` を記録してから、worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVWECEQG-worker-runtime-registry` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T10:37:43Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、blocking relation は 0 件。
- 以前の blocker だった `00001KVTNAY20` は `done` になり、orchestration branch に merge/validated 済み。
- `TicketOrchestrationPlanQuery` では過去の do_not_parallelize / waiting-capacity note と、今回の accepted plan `orch-plan-20260624-103655-3` を確認した。過去 blocker は解消済みとして扱う。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVWECEQG-worker-runtime-registry` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T10:38:50Z -->

## Implementation report

Implementation orchestration update:

- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVWECEQG-worker-runtime-registry` と branch `work/00001KVWECEQG-worker-runtime-registry` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVWECEQG-worker-registry` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVWECEQG-worker-registry at: 2026-06-24T10:57:04Z -->

## Implementation report

Implementation report:

実装概要:
- `crates/workspace-server/src/hosts.rs`
  - `WorkerRuntimeRegistry` を追加。
  - `WorkspaceWorkerRuntime` trait を runtime entry の抽象境界として再整理。
  - 既存 local Pod metadata 読み取りを `LocalPodRuntime` adapter として registry に登録。
  - `RuntimeSummary`, `RuntimeCapabilitySummary`, `WorkerCapabilitySummary`, `WorkerWorkspaceSummary`, typed `RuntimeDiagnostic` を追加。
  - API caller 由来の `host_id` / `worker_id` を backend 側で bounded identifier として検証。
  - raw session contents / socket path / metadata file path / raw workspace path を summary に出さないよう整理。
  - metadata の `workspace_root` は runtime workspace identity との照合にだけ使い、API には `current_workspace` summary として出す。
  - 他 workspace / workspace_root missing の Pod metadata は通常 worker overview から除外し、typed diagnostic のみ返す。
  - 先行 Ticket `00001KVTNAY20` の low-level launch 境界用 typed request/intent/result shape は維持。
- `crates/workspace-server/src/server.rs`
  - server state を `Arc<WorkerRuntimeRegistry>` に変更。
  - `/api/hosts`, `/api/workers`, `/api/hosts/{host_id}/workers` を registry 経由に変更。
  - `/api/runtimes` を追加し、runtime overview を返せるようにした。
  - invalid runtime identifier / unknown worker / unsupported capability の API error mapping を追加。
  - server tests を registry source / runtime overview / safe summary 仕様に更新。
- `crates/workspace-server/src/lib.rs`
  - registry/worker/capability 用 error variant を追加。
- `crates/workspace-server/src/repositories.rs`
  - shared typed diagnostic severity に合わせて更新。
- `web/workspace/src/lib/workspace-sidebar/types.ts`
  - runtime / capability / worker workspace summary 型を更新。
  - raw `workspace_root` / local `pod_name` 前提を削除。
- `web/workspace/src/lib/workspace-pages/WorkspacePage.svelte`
  - Host/Worker overview 表示を safe summary fields に合わせて更新。

Validation:
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `cd web/workspace && deno task check`: success
- `cd web/workspace && deno task build`: success
- `git diff --check`: success

Commit:
- `9bd15507 feat: add worker runtime registry overview`

Residual risks / notes:
- remote Host protocol / scheduler / stream proxy / worker control operation は non-goal のまま、typed capability/diagnostic で unsupported/pending を返す境界に留めた。
- API response から raw workspace/session/private path は出さない方針に寄せたため、UI は workspace path ではなく `current_workspace` scope/identity summary を表示する。
- `nix build .#yoi --no-link` は Cargo/Deno validation が通過済みだが、最終統合 confidence を上げる場合は Orchestrator 側判断で実行可能。

---

<!-- event: review author: yoi-reviewer-00001KVWECEQG-worker-registry at: 2026-06-24T11:01:43Z status: request_changes -->

## Review: request changes

Review result: request_changes

確認範囲:
- child worktree `/home/hare/Projects/yoi/.worktree/00001KVWECEQG-worker-runtime-registry` のみで read-only inspection。
- Ticket `item.md` / `thread.md` / orchestration plan、`371fd7c6..HEAD` の差分、主に `crates/workspace-server/src/hosts.rs` / `server.rs` / web 型・表示を確認。
- read-only のため、テスト・ビルド・`git diff --check` は実行していない。

良い点:
- `WorkspaceWorkerRuntime` trait、`WorkerRuntimeRegistry`、`LocalPodRuntime` が導入され、`WorkspaceApi` は `Arc<WorkerRuntimeRegistry>` を保持し、`/api/hosts` / `/api/workers` / `/api/hosts/{host_id}/workers` / 新規 `/api/runtimes` が registry 経由になっている。
- `HostSummary` / `WorkerSummary` は raw `workspace_root` / metadata path / socket path / raw session を直接返さない形に整理されており、UI 側も新しい safe summary shape に追随している。
- spawn/stop/proxy/stream は unsupported/reserved の型・diagnostic に留めており、scheduler / remote host protocol / raw session ingest には踏み込んでいない。

Blockers:
1. worker detail/lookup の id 解決が opaque id 境界を満たしていない。
   - `worker_id_for_pod()` は `pod_name` を sanitize/lowercase/truncate して `local-pod-*` を作る一方、`LocalPodRuntime::worker()` は `local-pod-` を strip した値をそのまま `pod_root/<pod_name>/metadata.json` のディレクトリ名として使っている。
   - 実 Pod 名に大文字、`.` などの置換対象文字、truncate 対象が含まれると、`list_workers()` が返した `worker_id` から同じ Worker を lookup できない。
   - Yoi の実 Pod 名には Ticket ID 由来の大文字を含むものがあり得るため現実的に壊れる。これは Ticket の「worker detail / lookup」「`pod_name` を external operation key にしない」「worker_id は backend が解決する opaque id」という要件に反する。
2. id の bounded/unique 性が full id に対して保証されていない。
   - `sanitize_identifier()` は fragment を最大 120 にするが、`local-pod-` / `local-` prefix 付与後の id は `validate_backend_identifier()` の `MAX_IDENTIFIER_LEN = 120` を超え得る。
   - sanitize による衝突（例: `foo.bar` と `foo-bar`）も避けられていない。将来 detail endpoint や operation routing を足した時に、一覧で返した id が lookup/validation で拒否される・別 Worker に衝突するリスクがある。

Required fixes:
- `worker_id` は返却後に確実に backend で再解決できる opaque/stable id にする。raw pod name を authority として外へ出さず、scan した metadata/dir entry から `generated_worker_id -> actual pod dir` を backend 内で照合する、または衝突しない hash/digest を含める。
- `MAX_IDENTIFIER_LEN` は prefix を含む full id に対して満たすようにし、生成側と検証側の不一致をなくす。
- 大文字 Ticket ID を含む Pod 名、`.` など sanitize される Pod 名、長い Pod 名、衝突ケースについて、`list_workers()` が返した `worker_id` を `registry.worker()` / `LocalPodRuntime::worker()` で引けるテストを追加する。

Non-blocking note:
- Ticket acceptance には `nix build .#yoi --no-link` も含まれている一方、coder 報告にはない。必要なら修正後に Orchestrator 判断で実施・明示すること。

---

<!-- event: implementation_report author: yoi-coder-00001KVWECEQG-worker-registry at: 2026-06-24T11:09:03Z -->

## Implementation report

Follow-up implementation report after review request_changes:

対応内容:
- `worker_id` を raw Pod dir 名から逆算する実装を廃止した。
  - `LocalPodRuntime::worker()` は Pod registry を backend 側で scan し、生成済み opaque `worker_id` と実 Pod dir を照合して解決する。
  - API caller が渡す `worker_id` は raw path / raw Pod name authority ではなく、backend-resolved identifier として扱う。
- `worker_id` / `host_id` の生成を bounded + collision-resistant に変更した。
  - prefix を含む full id が `MAX_IDENTIFIER_LEN = 120` 以下になるよう制御。
  - `sha2` digest の短縮 hex を suffix として含め、`foo.bar` / `foo-bar` や長大名 truncation による collision を避ける。
  - 生成 id は既存 validation を通る形式に限定。
- focused tests を追加・更新した。
  - uppercase Ticket-ID-like Pod name。
  - `.` / `@` / `#` 等の sanitized char。
  - `foo.bar` vs `foo-bar` collision case。
  - long/truncated names。
  - `list_workers()` が返す id を `registry.worker()` / `LocalPodRuntime::worker()` に渡して解決できること。
  - id が bounded / validation-compatible / duplicate-free であること。

変更ファイル:
- `Cargo.lock`
- `crates/workspace-server/Cargo.toml`
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`

Validation:
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `cd web/workspace && deno task check`: success
- `cd web/workspace && deno task build`: success
- `git diff --check`: success

Commit:
- `38d25582 fix: make worker runtime ids resolvable`

Residual risks / notes:
- `sha2.workspace = true` を `yoi-workspace-server` に追加したため、`Cargo.lock` の workspace-server dependency entry が更新された。
- raw session/private path leak、scheduler/remote protocol scope creep、`00001KVTNAY20` の low-level launch boundary は維持している。

---

<!-- event: review author: yoi-reviewer-00001KVWECEQG-worker-registry at: 2026-06-24T11:10:55Z status: approve -->

## Review: approve

Review result: approve

確認内容:
- child worktree のみで `9bd15507..HEAD` fix diff と必要箇所の現行実装を read-only 確認。
- ビルド/テストは read-only 指示のため再実行せず、coder 報告を参照。

判断:
- 以前の blocker 2件は解消されている。

根拠:
- `LocalPodRuntime::worker()` は `local-pod-` strip で raw Pod dir 名へ戻す実装をやめ、`pod_names()` で registry を scan し、各 actual Pod dir name から生成した `worker_id_for_pod()` と照合して backend 内部で解決する形になっている。これにより、大文字・記号・sanitize 対象文字を含む Pod 名でも list→lookup が成立する。
- `worker_id_for_pod()` / `host_id_for_workspace()` は `bounded_backend_identifier()` 経由になり、prefix を含む full id が `MAX_IDENTIFIER_LEN` 内に収まるよう body を切り詰め、SHA-256 digest suffix を付けて sanitize/truncation collision を避ける設計になっている。
- 追加テストで Ticket-like uppercase Pod 名、`.`/`-` collision、`#`/`@`、長い Pod 名、duplicate-free、validation-compatible、registry/runtime lookup roundtrip がカバーされている。
- raw metadata path / socket path / workspace root / raw session を overview API に出さない方針、registry 経由の read-only API、spawn/stop/proxy 非実装の unsupported 境界は維持されている。

Non-blocking notes:
- `worker_id` は sanitize 済み body を含むため完全な無意味 IDではないが、操作キーとしては backend 解決の opaque id になっており、今回の Ticket 意図には十分合っている。
- reported validation は通過とのことだが、reviewer では再実行していない。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T11:15:04Z -->

## Implementation report

Integration outcome:

- Coder `yoi-coder-00001KVWECEQG-worker-registry` が implementation branch `work/00001KVWECEQG-worker-runtime-registry` に実装を commit した。
- Reviewer `yoi-reviewer-00001KVWECEQG-worker-registry` は初回 review で `worker_id` opaque lookup / bounded unique id の blocker を指摘し、Coder が `38d25582 fix: make worker runtime ids resolvable` で修正した。
- Follow-up review は `approve`。`LocalPodRuntime::worker()` の backend 内部 scan/照合、bounded digest 付き id 生成、list→lookup roundtrip tests、raw path/session 非公開、unsupported capability boundary が確認された。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` で branch `work/00001KVWECEQG-worker-runtime-registry` を merge 済み。
  - merge commit: `1251c0ca merge: 00001KVWECEQG worker runtime registry`
  - implementation commit: `9bd15507 feat: add worker runtime registry overview`
  - fix commit: `38d25582 fix: make worker runtime ids resolvable`
- `sha2.workspace = true` 追加により `Cargo.lock` が変わったため、Nix package cargoHash を更新した。
  - package hash commit: `b975812c nix: update yoi cargo hash`

Validation in Orchestrator worktree:
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `cd web/workspace && deno task check`: success
- `cd web/workspace && deno task build`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success after updating `package.nix` cargoHash to `sha256-rR8mrMDg1CCXb3Y5MnhUqSd0vAsI5OStFQCRIrvx8fg=`

Notes:
- Initial `nix build .#yoi --no-link` failed only because cargoHash was stale after the new dependency; this was fixed and revalidated.
- Reviewer non-blocking note: generated `worker_id` still includes a sanitized body fragment, but as an operation key it is now backend-resolved opaque id and acceptable for this Ticket.

Next action:
- Mark Ticket done after this integration/validation evidence.
- Then stop related child Pods and remove only the child implementation worktree/branch.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T11:15:13Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Implementation is merged into the orchestration branch and validated.

Evidence:
- merge commit: `1251c0ca merge: 00001KVWECEQG worker runtime registry`
- package hash commit: `b975812c nix: update yoi cargo hash`
- reviewer result: approve after requested changes were fixed
- validation in `/home/hare/Projects/yoi/.worktree/orchestration` succeeded:
  - `cargo test -p yoi-workspace-server`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task build`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Closure is not performed here; this state records implementation completion after merge/validation.

---
