<!-- event: create author: "yoi ticket" at: 2026-06-24T19:51:56Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T19:55:29Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T19:55:29Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:11:58Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T20:52:56Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- この Ticket は legacy Plugin runtime redesign chain の 2 番目の concrete slice で、manifest validation / CLI diagnostics / docs cleanup に範囲が限定されている。
- outgoing `depends_on` は `00001KVXK0WD3` だが、`00001KVXK0WD3` は done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependent `00001KVXK0WDQ` はこの Ticket 完了後に進めるべき後続であり、この Ticket の acceptance blocker ではない。
- `TicketOrchestrationPlanQuery` には以前の `blocked_by 00001KVXK0WD3` があるが、prerequisite 完了により解消済みとして扱い、accepted plan `orch-plan-20260624-205226-2` を記録した。
- bounded context check で current orchestration branch の `crates/manifest/src/plugin.rs`, `crates/yoi/src/plugin_cli.rs`, `docs/development/plugin-development.md`, `docs/design/plugin-component-model.md`, `docs/design/plugin-packages.md` 周辺に raw `wasm` / `yoi-plugin-wasm-1` / transitional wording が残っていることを確認した。Ticket の scope はこれらの outward schema/diagnostic/docs 整理として十分に具体的。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。未解決 planning question は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVXK0WD3` は done。incoming dependent `00001KVXK0WDQ` は後続。
- Related Ticket: `00001KVXK0WD3` は done。active legacy runtime fallback removal completed with corrected merge commit `bedbb670`。
- Code/docs context: `crates/manifest/src/plugin.rs`, `crates/yoi/src/plugin_cli.rs`, `docs/development/plugin-development.md`, `docs/design/plugin-component-model.md`, `docs/design/plugin-packages.md`。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Legacy raw `wasm` / `yoi-plugin-wasm-1` Plugin runtime を external manifest/schema/CLI/docs surface でも rejected/retired として扱い、Component Model `wasm-component` を正の public/recommended runtime に一本化する。

Binding decisions / invariants:
- `00001KVXK0WD3` の active runtime removal を前提にする。raw-WASM execution fallback を戻さない。
- `runtime.kind = "wasm-component"` が positive accepted runtime kind。
- legacy raw `wasm` / `yoi-plugin-wasm-1` は new/current Plugin package として validation/inspection/check/list/show 上曖昧に active 表示しない。
- Component Model package の `check/list/show`、package discovery、digest, grants, Tool schema diagnostics は regress させない。
- Service / Ingress runtime、WebSocket driver、WIT/PDK/templates service event update は実装しない。

Requirements / acceptance criteria:
- Legacy raw `wasm` runtime manifest が validation/check で明確に rejected/unsupported になる。
- `yoi plugin check` は legacy package を invalid/unsupported として bounded diagnostic し、exit behavior が current check semantics と整合する。
- `yoi plugin list/show` は legacy package を有効/active Plugin として曖昧に表示しない。
- Component Model packages の check/list/show は通る。
- docs/design / docs/development / templates/examples から raw core-Wasm compatibility bridge 前提を削除または撤回済み方針に更新する。
- Manifest schema / static inspection / error message tests を更新する。

Implementation latitude:
- `PLUGIN_RUNTIME_WASM_KIND` / `PLUGIN_RUNTIME_WASM_ABI` constants を削除するか、legacy rejection diagnostic 用に internal-only に残すかは code style と tests に合わせてよい。
- Legacy fixture は削除または rejected fixture に変換してよい。
- CLI human/json diagnostic wording は bounded and clear であれば具体文言は coder が選んでよい。

Escalate if:
- Manifest parser cannot reject legacy runtime without breaking component packages。
- Plugin list/show architecture cannot represent rejected package diagnostics without broader API redesign。
- Implementing rejection requires Service/Ingress/WebSocket/WIT/PDK changes。
- Existing closed Ticket constraints force raw core-Wasm compatibility to remain externally supported。

Validation:
- `cargo test -p manifest`
- `cargo test -p yoi`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Add/run focused plugin CLI/check/list/show tests as appropriate。

Current code/docs map:
- Primary: `crates/manifest/src/plugin.rs`, `crates/yoi/src/plugin_cli.rs`。
- Secondary: `docs/development/plugin-development.md`, `docs/design/plugin-component-model.md`, `docs/design/plugin-packages.md`, plugin templates/examples only as needed。
- Avoid: `crates/pod` active runtime implementation unless minimal diagnostic alignment is needed; Service/Ingress/WebSocket/WIT/PDK event model。

Critical risks / reviewer focus:
- legacy package still appears active/eligible in CLI inspection。
- component package check/list/show regression。
- raw-WASM execution fallback being reintroduced。
- diagnostic wording that implies compatibility support remains。
- scope creep into later service runtime Tickets。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WDH-plugin-manifest-reject-legacy` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T20:53:45Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependency `00001KVXK0WD3` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260624-205226-2` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WDH-plugin-manifest-reject-legacy` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T20:54:43Z -->

## Implementation report

Implementation orchestration update:

- prerequisite `00001KVXK0WD3` が done になったため、`00001KVXK0WDH` を再 routing して受理した。
- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVXK0WDH-plugin-manifest-reject-legacy` と branch `work/00001KVXK0WDH-plugin-manifest-reject-legacy` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVXK0WDH-plugin-manifest-reject` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVXK0WDH-plugin-manifest-reject at: 2026-06-24T21:11:58Z -->

## Implementation report

Implementation report:

対象 Ticket:
- `00001KVXK0WDH` — Reject legacy Plugin runtime in manifest and CLI diagnostics

作業ブランチ / Worktree:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVXK0WDH-plugin-manifest-reject-legacy`
- Branch: `work/00001KVXK0WDH-plugin-manifest-reject-legacy`

変更内容:
- `crates/manifest/src/plugin.rs`
  - legacy raw `wasm` / `yoi-plugin-wasm-1` runtime を manifest validation で reject。
  - `wasm-component` を現行 public runtime としてドキュメントコメントを更新。
  - raw-WASM runtime module 読み込み helper を削除。
  - legacy manifest rejection のテストを追加。
- `crates/yoi/src/plugin_cli.rs`
  - CLI テスト fixture を `wasm-component` package へ移行。
  - `yoi plugin check` が legacy raw-WASM package を `rejected` として返すテストを追加。
  - `list/show` inspection 上で legacy package が active/eligible 表示されないことをテスト。
- `crates/pod/src/feature/plugin.rs`
  - public manifest constant 依存を外し、legacy runtime 名は内部診断用 const に限定。
  - 既存の runtime fallback rejection 経路を維持。
- `docs/development/plugin-development.md`
  - raw-WASM runtime を「互換/推奨」ではなく retired/rejected として記述更新。
  - `wasm-component` を supported/public runtime として明示。
- `docs/design/plugin-component-model.md`
  - Component Model runtime を現行 supported runtime として更新。
  - raw-WASM は bounded diagnostic 用の rejected metadata として整理。
- `docs/design/plugin-packages.md`
  - package runtime 記述を `wasm-component` 中心に更新。
  - legacy raw-WASM package は active/eligible ではなく rejected として表示されることを明記。

Validation:
- `cargo fmt --check`: success
- `cargo test -p manifest`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `cargo test -p pod legacy_raw_wasm_runtime_is_rejected_without_fallback_execution`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success
  - dirty tree warning は未コミット検証時点の通常 warning。ビルド自体は成功。

Commit:
- `6086099fe4807b5b12c8d13333f5544a4174e3d9 feat: reject legacy plugin runtime manifests`

Residual risks / notes:
- 既知の残リスクなし。
- raw-WASM execution fallback は再導入していない。
- legacy runtime 名は manifest/pod 内部の rejected diagnostic fixture/判定用としてのみ残している。
- Service/Ingress runtime、WebSocket driver、WIT/PDK/templates の service event 更新、broader runtime redesign は実施していない。

---

<!-- event: review author: yoi-reviewer-00001KVXK0WDH-plugin-manifest-reject at: 2026-06-24T21:18:53Z status: approve -->

## Review: approve

Review result: approve

確認範囲:
- child worktree `/home/hare/Projects/yoi/.worktree/00001KVXK0WDH-plugin-manifest-reject-legacy`
- branch `work/00001KVXK0WDH-plugin-manifest-reject-legacy`
- HEAD `6086099fe4807b5b12c8d13333f5544a4174e3d9`
- 実履歴上の親 `ef1d8d9a` → HEAD の差分をレビューした。指定 base `187e6426` はこの worktree では object missing だったが、HEAD の直前 commit は `ticket: accept plugin manifest rejection task` で、レビュー対象差分として妥当と判断した。

承認根拠:
- Ticket / IntentPacket の要件と照合済み。
- `crates/manifest/src/plugin.rs`:
  - legacy `wasm` / `yoi-plugin-wasm-1` は public const ではなく internal rejection 用 const になっている。
  - `validate_manifest` で `kind = "wasm"` を `legacy raw wasm ... retired; use wasm-component` として `Api/Manifest` diagnostic 付きで拒否している。
  - `wasm-component` は正の runtime kind として `component` / `world` validation を維持している。
  - legacy rejection test が追加され、discovery package には入らず diagnostic になることを確認している。
- `crates/yoi/src/plugin_cli.rs`:
  - `plugin check` は manifest/discovery rejection を `status: "rejected"` として bounded diagnostic に載せ、既存 check semantics 通り rejected で Err を返す。
  - list/show snapshot は discovery/resolution diagnostics から rejected item を構成し、legacy package を active/eligible として扱わない。
  - component fixture は `wasm-component` / `plugin.component.wasm` に更新され、legacy fixture は rejected fixture に変換されている。
- `crates/pod/src/feature/plugin.rs`:
  - manifest crate の legacy runtime public const 依存は削除済み。
  - legacy name は internal diagnostic/rejection matching のみ。
  - `PluginInstanceRuntime::new` は legacy `wasm` を即エラーにし、raw-WASM fallback 実行は戻っていない。
- docs:
  - `docs/development/plugin-development.md`
  - `docs/design/plugin-component-model.md`
  - `docs/design/plugin-packages.md`
  で raw core-Wasm compatibility 前提は撤回され、`wasm-component` が supported/public runtime と明記されている。
- `rg 'plugin\.wasm|kind = "wasm"|abi = "yoi-plugin-wasm-1"'` で残存箇所を確認。残りは rejection docs/tests/diagnostics と component build artifact 名の文脈のみで、active public compatibility 表現は見当たらない。
- 差分対象は manifest / CLI / pod diagnostic alignment / docs の 6 files に限定され、Service/Ingress/WebSocket/WIT/PDK runtime redesign への scope creep は確認していない。

Validation:
- reviewer は read-only 制約のため `cargo test` / `cargo check` / `nix build` は再実行していない。
- reviewer 側で `git diff --check HEAD^..HEAD`: success。
- Coder reported validation (`cargo fmt --check`, `cargo test -p manifest`, `cargo test -p yoi`, `cargo check -p yoi`, focused pod test, `nix build .#yoi --no-link`) は実装内容と整合しており、追加で疑う材料はない。

Non-blocking note:
- 親から指定された review base hash `187e6426` は誤記/存在しない hash だった。実レビューは child branch の実 parent `ef1d8d9a` を base として行われた。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T21:20:36Z -->

## Implementation report

Integration outcome:

- Coder `yoi-coder-00001KVXK0WDH-plugin-manifest-reject` が implementation branch `work/00001KVXK0WDH-plugin-manifest-reject-legacy` に実装を commit した。
  - implementation commit: `6086099f feat: reject legacy plugin runtime manifests`
- Reviewer `yoi-reviewer-00001KVXK0WDH-plugin-manifest-reject` は read-only review で `approve`。legacy `wasm` / `yoi-plugin-wasm-1` manifest rejection、CLI check/list/show diagnostic behavior、component package non-regression、pod diagnostic alignment、docs update、scope creep なしを確認した。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` で branch `work/00001KVXK0WDH-plugin-manifest-reject-legacy` を merge 済み。
  - merge commit: `449745ee merge: 00001KVXK0WDH plugin manifest rejection`

Implemented behavior:
- `crates/manifest/src/plugin.rs`:
  - legacy raw `wasm` / `yoi-plugin-wasm-1` runtime を manifest validation で rejected diagnostic にする。
  - `wasm-component` を supported public runtime として維持。
- `crates/yoi/src/plugin_cli.rs`:
  - `yoi plugin check` は legacy package を `status: "rejected"` として bounded diagnostic に載せ、既存 check semantics 通り失敗扱いにする。
  - list/show fixture は legacy package を active/eligible として表示しない。
- `crates/pod/src/feature/plugin.rs`:
  - legacy runtime public constant dependency を外し、legacy runtime name は internal diagnostic/rejection matching のみに限定。
  - raw-WASM fallback execution は再導入していない。
- docs:
  - `docs/development/plugin-development.md`, `docs/design/plugin-component-model.md`, `docs/design/plugin-packages.md` で raw core-Wasm compatibility 前提を撤回し、`wasm-component` を supported/public runtime と明記。

Validation in Orchestrator worktree:
- `cargo fmt --check`: success
- `cargo test -p manifest`: success
- `cargo test -p yoi`: success
- `cargo check -p yoi`: success
- `cargo test -p pod legacy_raw_wasm_runtime_is_rejected_without_fallback_execution --lib`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Notes:
- Reviewer non-blocking note: Orchestrator から渡した review base `187e6426` は誤記で、child branch の実 parent `ef1d8d9a` を base として review された。review scope は適切だった。
- Follow-up `00001KVXK0WDQ` は this Ticket completion により dependency-unblocked and can be re-routed next.

Next action:
- Mark Ticket done after this integration/validation evidence.
- Then stop related child Pods and remove only the child implementation worktree/branch.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T21:20:45Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Implementation is merged into the orchestration branch and validated.

Evidence:
- merge commit: `449745ee merge: 00001KVXK0WDH plugin manifest rejection`
- reviewer result: approve
- validation in `/home/hare/Projects/yoi/.worktree/orchestration` succeeded:
  - `cargo fmt --check`
  - `cargo test -p manifest`
  - `cargo test -p yoi`
  - `cargo check -p yoi`
  - `cargo test -p pod legacy_raw_wasm_runtime_is_rejected_without_fallback_execution --lib`
  - `git diff --check`
  - `nix build .#yoi --no-link`

Closure is not performed here; this state records implementation completion after merge/validation.

---
