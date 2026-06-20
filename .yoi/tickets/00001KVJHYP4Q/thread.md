<!-- event: create author: "yoi ticket" at: 2026-06-20T13:01:37Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-20T13:02:36Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-20T13:02:36Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T13:28:19Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T13:29:10Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Workspace Dashboard Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body は Plugin instance model / registry、new Component instance world、legacy Tool adapters、manifest/static validation、plugin check/list/show reporting、Service/Ingress lifecycle/status、Ingress test path、per-surface grants、docs/templates/PDK updates、validation を詳細に定義している。
- 未解決 relation blocker はない。
- 現在 queued はこの Ticket のみ、inprogress は 0 件、spawned child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は plugin / wasm-component / service / ingress / lifecycle / grants / runtime architecture だが、Ticket は no hidden context injection、ToolRegistry run-stability、legacy Tool compatibility、no ambient WASI network/socket、per-surface grants、host-mediated outputs を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVJHYP4Q` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVJHYP4Q)`: no blockers。
- `TicketOrchestrationPlanQuery(00001KVJHYP4Q)`: no previous plan records; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `7f06e656`。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods are self/peers only; spawned children 0。
  - no matching implementation branch/worktree。

IntentPacket:

Intent:
- Move Plugin runtime semantics from per-Tool artifact execution to host-managed `PluginInstance` / `PluginInstanceRegistry`。
- Treat Tool / Service / Ingress as surfaces of the same Plugin instance, sharing instance state/config/diagnostics while preserving explicit authorization and ordinary visible output paths。
- Preserve existing Tool-only component/raw wasm Plugin packages through compatibility adapters。

Binding decisions / invariants:
- Existing Tool Plugin packages must continue to work through instance registry compatibility path。
- Tool execution remains model/user initiated and returns through ordinary Tool result/history path。
- Service/Ingress must not secretly call model Tools or mutate context/history directly。
- Plugin outputs/events must use Tool results or explicit durable/visible host-mediated paths; no hidden context injection。
- Tool schemas remain run-stable and model-visible only through normal ToolRegistry construction。
- Per-surface grants are independent: Tool, Service, Ingress grants must be validated separately; sharing an instance must not bypass authorization。
- Host APIs remain separately grant-gated。
- No raw ambient WASI network/socket authority。
- Ingress events are bounded typed untrusted inputs。
- If a safe host action path does not exist, expose diagnostics/status rather than inventing unsafe paths。

Requirements / acceptance criteria:
- Add `PluginInstanceRegistry` or equivalent host-managed instance boundary。
- ToolRegistry dispatch goes through Plugin instance handles。
- Add new Component instance world/resource files and Rust PDK support。
- Add legacy Tool component/raw wasm adapters behind the instance registry。
- Extend manifest/static validation for Service/Ingress declarations and runtime compatibility。
- Update `yoi plugin check/list/show` reporting for legacy Tool-only vs instance-capable packages and rejected surfaces。
- Add host-managed start/status/stop lifecycle and bounded diagnostics。
- Add Ingress dispatch API and at least one bounded in-process ingress delivery test path。
- Validate Tool/Service/Ingress grants independently。
- Update docs/templates for instance-oriented authoring。
- Focused tests cover manifest validation, legacy compatibility, instance state persistence across Tool calls, Tool/Ingress shared instance dispatch, grant denial, timeout/trap/failure diagnostics。
- Validation includes `cargo fmt --check`, relevant tests/checks, `git diff --check`, `yoi ticket doctor`, and `nix build .#yoi --no-link`。

Escalate if:
- The instance boundary cannot be implemented without broad Worker/ToolRegistry redesign beyond Ticket scope。
- Preserving legacy Tool runtime while adding instance runtime would substantially distort architecture。
- Safe Service/Ingress host action semantics require a product decision not already specified。
- WIT/PDK interface shape requires a compatibility-breaking public API decision beyond this Ticket。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T13:29:23Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_instance_lifecycle field: state -->

## State changed

Ticket body/thread, relation metadata, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded Plugin instance lifecycle context were checked. There is no unresolved blocking dependency, no inprogress/capacity blocker, and no missing planning decision. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---
