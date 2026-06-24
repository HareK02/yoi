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

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:12:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T21:22:37Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- `00001KVXK0WDQ` は Service lifecycle / ingress queue runtime の concrete slice で、WebSocket driver、output command model、WIT/PDK/templates update を non-goal として後続に分離している。
- outgoing `depends_on` は `00001KVXK0WDH` だが、`00001KVXK0WDH` は done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependents (`00001KVXK0WDX`, `00001KVXK0WE4`) はこの Ticket 完了後に進めるべき後続であり、この Ticket の acceptance blocker ではない。
- bounded context check で `crates/pod/src/feature/plugin.rs` の current `PluginInstanceRegistry`, `PluginInstanceHandle`, `PluginIngressEvent`, `PluginIngressDispatchReport`, `ComponentInstanceRuntime` lifecycle methods を確認した。Ticket は in-process service lifecycle / bounded ingress queue / serial dispatch / diagnostics に収まっており、残る不確実性は local implementation に閉じる。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。未解決 planning question は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVXK0WDH` は done。routing 前 plan は historical blocked_by `00001KVXK0WDH` のみで、prerequisite 完了により解消済み。accepted plan `orch-plan-20260624-212209-2` を記録済み。
- Related Tickets: `00001KVXK0WD3` / `00001KVXK0WDH` は done。
- Code context: `crates/pod/src/feature/plugin.rs` の service/ingress registration, instance registry, start/status/stop/handle-ingress, component runtime tests。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Plugin Service を host-managed lifecycle と bounded ingress queue を持つ in-process runtime として扱い、`start()` を initialization-only にし、後続 ingress events を queue 経由で serial dispatch できるようにする。

Binding decisions / invariants:
- Existing Tool Plugin execution は request-response operation として維持し、service queue に巻き込まない。
- v0 dispatch は per-plugin serial dispatch。concurrent per-plugin event execution は non-goal。
- Queue は bounded。full / timeout / failed service / stop 中 event / invalid event は typed error / diagnostic として扱う。
- `start()` は long-running loop / polling loop / recv loop を担わない。
- Durable cross-process event queue は non-goal。まず host-managed in-process queue/lifecycle として実装する。
- WebSocket driver と output command model は後続 Tickets (`00001KVXK0WE4`, `00001KVXK0WDX`) に残す。
- Component Model-only runtime authority and manifest rejection from prerequisites must not regress.

Requirements / acceptance criteria:
- Service Plugin instance が ready/starting/running/stopping/stopped/failed 相当の lifecycle state を持つ。
- `start()` return 後も ingress event を queue 経由で配送できる。
- Ingress event has source / ingress name / payload / created_at / attempt / correlation id.
- Queue depth / lifecycle state / last error / dispatch counters are visible in status diagnostics.
- Unit tests cover lifecycle start/stop/failure, bounded queue full, serial dispatch, timeout/failure diagnostics, stop-time event rejection, Tool execution regression.
- `cargo test -p pod`, `cargo check -p yoi`, `git diff --check`, `nix build .#yoi --no-link` are validation targets.

Implementation latitude:
- Choose exact structs/enums/names and whether queue processing is sync-step driven or background worker, as long as start returns promptly and dispatch remains host-managed/serial.
- Existing `PluginInstanceRegistry` may be extended or refactored if authority boundaries remain clear.
- Tests may use existing `test-ingress` runtime or focused component fixtures.

Escalate if:
- Implementation requires durable cross-process queue or scheduler semantics.
- WebSocket driver or output command model must be implemented to make lifecycle tests pass.
- Component Tool execution has to be routed through service queue.
- Host API grant/runtime authority or Component Model manifest behavior needs redesign.

Validation:
- `cargo test -p pod`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Focused tests for service lifecycle/queue may be run during development.

Current code map:
- Primary: `crates/pod/src/feature/plugin.rs`。
- Secondary only if needed: manifest service/ingress declarations and docs comments.
- Avoid: WebSocket driver, output command model, WIT/PDK/templates event update, remote runtime protocol, durable cross-process queue.

Critical risks / reviewer focus:
- accidentally making Service runtime a scheduler/durable queue.
- Tool execution regression or Tool path accidentally using Service queue.
- unbounded queue or hidden background loop.
- missing stop/failure/timeout diagnostics.
- scope creep into WebSocket/output commands/WIT-PDK.

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WDQ-plugin-service-lifecycle` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T21:23:16Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependency `00001KVXK0WDH` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260624-212209-2` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WDQ-plugin-service-lifecycle` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T21:24:10Z -->

## Implementation report

Implementation orchestration update:

- prerequisite `00001KVXK0WDH` が done になったため、`00001KVXK0WDQ` を再 routing して受理した。
- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVXK0WDQ-plugin-service-lifecycle` と branch `work/00001KVXK0WDQ-plugin-service-lifecycle` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVXK0WDQ-service-lifecycle` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVXK0WDQ-service-lifecycle at: 2026-06-24T21:43:56Z -->

## Implementation report

Implementation report:

対象 Ticket:
- `00001KVXK0WDQ` — Define Plugin Service lifecycle and ingress queue runtime

変更ファイル:
- `crates/pod/src/feature/plugin.rs`

主な実装:
- Plugin Service の host-managed lifecycle を追加。
  - `Ready / Starting / Running / Stopping / Stopped / Failed`
- in-process bounded ingress queue を追加。
  - v0 は per-plugin serial dispatch。
  - Tool Plugin 実行は従来通り request-response のまま維持。
- `PluginIngressEvent` に以下を追加。
  - `source`
  - `ingress_name`
  - `payload`
  - `created_at`
  - `attempt`
  - `correlation_id`
- typed ingress dispatch error / diagnostic を追加。
  - invalid event
  - queue full
  - service unavailable / failed / stopped
  - dispatch timeout
  - dispatch failure
- status diagnostics に以下を追加。
  - lifecycle state
  - queue depth / capacity
  - last error
  - dispatch counters
- Unit tests を追加・更新。
  - lifecycle start/stop/failure
  - bounded queue full
  - serial dispatch
  - timeout diagnostic
  - dispatch failure diagnostic
  - stopped service rejection
  - invalid event rejection
  - Tool execution regression

Commit:
- `4e138b7e365d940bf31397aa64cf14a58b6bc51a feat: add plugin service ingress queue`

Validation:
- `cargo test -p pod feature::plugin::tests`: success (`73 passed`)
- `cargo check -p yoi`: success
- `git diff --check`: success
- `git diff --check HEAD~1 HEAD`: success
- `nix build .#yoi --no-link`: success
- `cargo test -p pod`: failed in existing prompt guidance assertions outside this Ticket:
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
  - failure: `assertion failed: rendered.contains("worktree status, diff, and test results")`

Residual risks / notes:
- Full `cargo test -p pod` remains blocked by unrelated prompt/resource snapshot assertions; Plugin-focused tests pass.
- v0 dispatch timeout is implemented as elapsed check after synchronous dispatch completion and typed diagnostic. Force-cancelling executing Wasm in a separate worker is broader runtime redesign and remains non-goal.

---
