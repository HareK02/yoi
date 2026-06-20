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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T08:43:53Z -->

## Implementation report

Implementation orchestration start:
- User explicitly authorized parallel work for Tickets without blockers。
- Acceptance records were committed on Orchestrator branch: `7e35721a ticket: accept dashboard console tui refactor`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHX0WBE-dashboard-console-tui-boundary` on branch `impl/00001KVHX0WBE-dashboard-console-tui-boundary` at `7e35721a`。
- Spawned Coder Pod `yoi-coder-00001KVHX0WBE` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, `yoi panel` rename/alias additions without escalation, behavior changes to Ticket/Pod/Panel authority, merge, Ticket close, and cleanup。

Next action:
- Let Dashboard/Console Coder proceed in parallel with the MCP tool-registration review loop. Inspect implementation report and route to Reviewer when ready。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T09:13:21Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHX0WBE`.

Implementation commit:
- `5415a947 tui: introduce dashboard console boundaries`

Changed areas reported:
- Introduced `crates/tui/src/dashboard/` as Dashboard boundary:
  - `mod.rs` for Dashboard state/action/lifecycle entrypoint。
  - `render.rs` for render/list/layout composition。
  - `tests.rs` for Dashboard-focused tests。
- Introduced `crates/tui/src/console/` as the single-Pod Console boundary。
- Updated `crates/tui/src/lib.rs` routing:
  - `LaunchMode::Panel => dashboard::launch(...)`。
  - Console launch modes route through `console::*`。
  - No `single_pod::run_panel` path remains。
- Preserved `yoi panel` command name and did not add `yoi dashboard`。
- Updated help/docs terminology: Dashboard = `yoi panel` workspace cockpit/action surface; Console = single-Pod chat/client surface; TUI = implementation umbrella。
- Updated tests/help expectations and brittle profile-default assertions surfaced by validation。

Coder validation reported:
- `cargo test -p tui`: passed。
- `cargo test -p yoi`: passed。
- `cargo check --workspace --all-targets`: passed。
- `cargo fmt --check`: passed。
- `git diff --check`: passed。
- `git diff --cached --check`: passed。
- Targeted help/grep checks for Dashboard/Console terminology and no `yoi dashboard` alias: passed。
- `nix build .#yoi --no-link` not run because package/source filters were not changed。

Known risks / deferrals:
- Internal `workspace_panel` naming remains in lower-level model types to avoid broad unrelated churn; user-facing docs/help and TUI entry/module boundaries now use Dashboard/Console terminology。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `5415a947`。
- Diff from acceptance `7e35721a..HEAD` is one implementation commit touching 12 files with a large split/rename: `single_pod.rs -> console/mod.rs`, `multi_pod.rs -> dashboard/mod.rs`, new `dashboard/render.rs`, new `dashboard/tests.rs`, docs/help/prompt updates。
- `git diff --check 7e35721a..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on behavior preservation, `yoi panel` command preservation/no alias, Dashboard/Console/TUI terminology, entrypoint/module boundary, large-file split quality, tests, and whether changed prompt/help wording is appropriate。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T09:14:08Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHX0WBE-r1` against implementation branch `impl/00001KVHX0WBE-dashboard-console-tui-boundary`。
- Review target commit: `5415a947 tui: introduce dashboard console boundaries`。
- Review baseline: `7e35721a`。
- Reviewer task focuses on behavior preservation, `yoi panel` command preservation / no `yoi dashboard` alias, Dashboard/Console/TUI terminology, entrypoint/module boundaries, large-file split quality, prompt/help/doc wording, tests, and package/source-filter impact。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---

<!-- event: review author: yoi-reviewer-00001KVHX0WBE-r1 at: 2026-06-20T09:22:25Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket contract / Orchestrator IntentPacket。
- Implementation diff: `7e35721a..5415a947`。
- 主な対象: `crates/tui/src/lib.rs`, `crates/tui/src/dashboard/mod.rs`, `crates/tui/src/dashboard/render.rs`, `crates/tui/src/dashboard/tests.rs`, `crates/tui/src/console/mod.rs`, `crates/yoi/src/main.rs`, `crates/yoi/src/ticket_cli.rs`, `crates/tui/src/spawn.rs`, README / TUI README / development docs / prompt resource terminology changes。

Positive findings:
- `yoi panel` remains the CLI command。
- `yoi dashboard` alias は見つからない。
- `LaunchMode::Panel` は Dashboard boundary を通るようになり、`single_pod::run_panel` path は見つからない。
- `single_pod.rs` / `multi_pod.rs` は `console/` / `dashboard/` に置換されている。

Blocking issues:

1. Recoverable Dashboard open failures now exit `yoi panel`。
   - Path: `crates/tui/src/dashboard/mod.rs`。
   - Dashboard から Pod を開き、nested Console open が spawn failure/cancel など recoverable error を返すと、`app.finish_open(...)` 後に Dashboard fullscreen を抜けて function が `Ok(())` を返すように見える。
   - Baseline `single_pod::run_panel` では recoverable nested-open failures は既存 Panel/Dashboard state に折り返され、Panel loop は継続した。
   - Cancelled/failed Console open が workspace Dashboard を閉じるのは、Ticket の behavior preservation / Panel action model preservation 要件に反する。

2. Successful return from Console discards live Dashboard state。
   - Path: `crates/tui/src/dashboard/mod.rs`。
   - `console::run_pod_name_nested(...)` 成功後、`app.finish_open(...)` の後に `app = load_app(runtime_command.clone()).await?;` で fresh app に置き換えている。
   - これは baseline と `finish_open` の intent に反し、selection context、composer draft/target、notices、diagnostics、Dashboard-local UI state を失う可能性がある。
   - Ticket は boundary/terminology refactor であり、Dashboard state preservation は reviewer focus。Live `DashboardApp` を保持し、`finish_open(...)` + existing reload/reconcile mechanism を使うべき。

Required fix:
- Successful nested Console return と recoverable nested Console failure の両方で、existing `DashboardApp` を保持して Dashboard loop を継続する。
- `finish_open(...)` と既存 reload mechanism を使い、function return / fresh app replacement で Dashboard session state を捨てない。
- Regression tests を追加または既存 tests を拡張し、nested open success/failure が Dashboard state/draft/selection/diagnostic を壊さず loop 継続することを可能な範囲で確認する。

Non-blocking concerns:
- `dashboard/mod.rs` はまだ大きく、action/lifecycle/snapshot/model/diagnostics/E2E fixture helpers が混在している。追加分割は follow-up でもよいが、今回の blocker fix で触るなら改善可。
- `console/mod.rs` に Dashboard/fullscreen helper concepts が少し残る。terminal-mode bridge として許容可能だが、Console focus は維持すること。
- `workspace_panel` / `WorkspacePanel*` naming は internal model names と見えるため blocking ではない。

Reviewer validation:
- `cargo fmt --check`: passed。
- `git diff --check 7e35721a..HEAD`: passed。
- `cargo test -p tui`: passed。
- `cargo test -p yoi`: passed。
- `cargo check --workspace --all-targets`: passed。
- Targeted help/grep checks: no `yoi dashboard` alias or old `single_pod::run_panel` route found。
- `nix build .#yoi --no-link` not run because no package/source-filter concern found。

Worktree status at review end: tracked source tree clean; ignored `target/` only。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T09:30:11Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVHX0WBE`.

New fix commit:
- `135343a2 tui: preserve dashboard after console return`

Branch commits now:
- `135343a2 tui: preserve dashboard after console return`
- `5415a947 tui: introduce dashboard console boundaries`

Fix summary:
- `crates/tui/src/dashboard/mod.rs`:
  - Added `finish_nested_console_open(...)` helper。
  - Successful nested Console return now calls `app.finish_open(...)` and continues the Dashboard loop with the same `DashboardApp`。
  - Recoverable nested Console failures now call `app.finish_open(...)` and continue the Dashboard loop instead of leaving fullscreen / exiting。
  - Non-recoverable errors still leave Dashboard fullscreen and bubble the error。
  - Removed fresh `load_app(...)` replacement after Console return, preserving selection, composer draft, diagnostics, notices, and other local Dashboard state。
- `crates/tui/src/dashboard/tests.rs`:
  - Added regression coverage for nested Console success continuing without dropping Dashboard state。
  - Added regression coverage for recoverable nested Console failure continuing without dropping Dashboard state。
  - Added regression coverage for non-recoverable nested Console failure bubbling without incorrectly finishing/mutating Dashboard open state。

Coder validation reported as passing:
- `cargo fmt --check`
- `git diff --check`
- `git diff --check 7e35721a..HEAD`
- `cargo test -p tui`
- `cargo test -p yoi`
- `cargo check --workspace --all-targets`
- Targeted grep/help checks for Dashboard/Console/TUI terminology, `LaunchMode::Panel => dashboard::launch(...)`, no `single_pod::run_panel`, and no `yoi dashboard` alias。

Orchestrator evidence checked before r2 review:
- Implementation worktree is clean。
- HEAD is `135343a2`。
- Fix diff from `5415a947..HEAD` touches `crates/tui/src/dashboard/mod.rs` and `crates/tui/src/dashboard/tests.rs`, about 95 insertions / 15 deletions。
- Overall `git diff --check 7e35721a..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r2 verification of Dashboard state preservation and final acceptance criteria。

---
