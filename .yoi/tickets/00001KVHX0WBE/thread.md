<!-- event: create author: LocalTicketBackend at: 2026-06-20T06:55:49Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-20T06:55:57Z -->

## Intake summary

ユーザー合意に基づき、Dashboard / Console 呼称導入、`yoi panel` Dashboard 境界、単一 Pod Console 境界、`multi_pod.rs` の Dashboard module への移設・分割、docs/help/tests 更新を 1 つの concrete implementation Ticket として整理した。readiness は `implementation_ready`。risk flags は stop gate ではなく reviewer focus として扱う。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-20T06:55:57Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件・受け入れ条件・binding decisions・validation が揃っており、Orchestrator が実装 routing 可能。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T08:30:58Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T08:31:27Z -->

## Decision

Routing decision: waiting_capacity

Panel Queue により routing 対象として確認したが、現在 `00001KVHR3WS6` が `inprogress` で MCP tool registration の Coder/Reviewer 作業中のため、この Dashboard / Console refactor Ticket は実装開始せず queued のまま保持する。

Evidence:
- Ticket body / relations を確認。未解決 blocker relation はない。
- Orchestrator worktree は clean。
- inprogress は `00001KVHR3WS6` のみ。
- matching branch/worktree は存在しない。
- 現在 active child Pods は `yoi-coder-00001KVHR3WS6` と `yoi-reviewer-00001KVHR3WS6-r1`。

Next:
- 現在の `00001KVHR3WS6` 作業が完了して implementation capacity が空いた後、改めて reroute する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T08:42:24Z -->

## Decision

Routing decision: implementation_ready_parallel

Reason:
- User directive: 「blocker無いなら並列にやっちゃえよ」。この指示により、未解決 blocker がない実装 Ticket は現在の MCP review loop と並行して開始する方針に更新した。
- Ticket body は Dashboard / Console / TUI の呼称導入、`yoi panel` command維持、不要 alias 不追加、Dashboard / Console entrypoint 境界、`multi_pod.rs` の Dashboard module への移設・分割、docs/help/tests更新、validation を実装可能な粒度で定義している。
- `TicketRelationQuery(00001KVHX0WBE)`: 未解決 blocker relation なし。
- 現在 `00001KVHR3WS6` は inprogress だが domain は MCP tool registration であり、この Dashboard/Console TUI refactor と直接 conflict しない。別 worktree / branch / sibling Coder Pod で並列化できる。
- Orchestrator worktree は clean、matching branch/worktree はなし。
- Risk flags は ux-naming / module-boundary / public-cli / test-coverage だが stop gate ではなく reviewer focus。Ticket は `yoi panel` command維持、`yoi dashboard` 等の不要 alias 不追加、Dashboard は scheduler/backend ではない、挙動変更は目的にしない、tests維持を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHX0WBE` body / thread / artifacts。
- Current workspace state:
  - Orchestrator worktree clean at `8ce4fcde`。
  - inprogress: `00001KVHR3WS6` only。
  - queued includes this Ticket and downstream MCP Tickets。
  - visible spawned children are MCP tool-registration coder/reviewer; no Dashboard/Console branch/worktree exists。

IntentPacket:

Intent:
- Introduce user-facing terminology: Dashboard = `yoi panel`, Console = single-Pod chat/client surface, TUI = terminal UI implementation umbrella。
- Refactor TUI module boundaries so Dashboard is not routed through Console/single-Pod module entrypoints, and `multi_pod.rs` responsibilities move toward a Dashboard module boundary。
- Preserve behavior. The objective is naming / module boundary / maintainability refactor, not functional change。

Binding decisions / invariants:
- Keep `yoi panel` command name。
- Do not add `yoi dashboard` or other unnecessary compatibility aliases。
- Dashboard is workspace-level cockpit/action surface, not scheduler/backend。
- Console is single-Pod chat/client surface and is not a subordinate Dashboard mode。
- TUI is implementation layer umbrella, not a replacement mode name。
- Dashboard may open a Pod Console, but bridge should be narrow and readable。
- Avoid behavior changes to Ticket authority, Pod lifecycle, Orchestrator handoff, Panel action model, input model, or rendering semantics unless explicitly required and reported。
- Keep/port existing tests; exact string tests should be updated to Dashboard/Console terminology where appropriate。

Requirements / acceptance criteria:
- `yoi panel` help/docs describe Dashboard。
- Single Pod UI help/docs describe Console。
- Dashboard entrypoint lives in Dashboard module side; `LaunchMode::Panel` does not flow through a Console module entrypoint like `single_pod::run_panel`。
- Console module focuses on single-Pod chat/connect/spawn/resume。
- `multi_pod.rs` giant file state is materially improved by moving/splitting responsibilities into Dashboard module boundaries。
- Render/list/layout, action/lifecycle, diagnostics/e2e, tests boundaries are reviewer-readable。
- Existing workspace panel/action model remains intact; `yoi panel` remains Ticket-centric workspace cockpit and not scheduler/backend。
- Validation includes `cargo test -p tui`, `cargo test -p yoi`, `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and targeted grep/help checks as needed。

Escalate if:
- Dashboard/Console split is insufficient for a third user-facing surface。
- `yoi panel` command rename or `yoi dashboard` alias seems necessary。
- Module split requires behavior changes to Ticket state authority, Pod lifecycle, Orchestrator handoff, or user-visible behavior beyond naming/boundary。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T08:42:30Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_dashboard_console_tui_boundary field: state -->

## State changed

User explicitly authorized parallel implementation when no blocker exists. Ticket body/thread, relation metadata, prior waiting-capacity note, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded Dashboard/Console TUI context were checked. There is no unresolved blocking dependency, no matching worktree/branch, and no missing planning decision. Accepting this queued Ticket for parallel implementation before worktree/Pod side effects.

---
