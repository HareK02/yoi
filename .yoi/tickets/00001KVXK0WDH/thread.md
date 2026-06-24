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
