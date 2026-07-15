<!-- event: create author: "yoi ticket" at: 2026-07-15T16:18:25Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-15T16:18:40Z -->

## Plan

Background:
- `.yoi/ticket.config.toml` currently owns Ticket backend root/provider, Ticket record language, orchestration defaults, and fixed role launch config.
- Workspace backend / Runtime / Worker Ticket tools are moving toward Workspace-owned Ticket authority, so a separate Ticket-specific config file is no longer the right durable settings boundary.
- Local backend operational settings such as listen address, DB path, runtime store, and remote runtime endpoint should remain local/backend settings, not project Ticket policy.

Requirements:
- Introduce or reuse a tracked Workspace project settings file as the durable authority for Ticket configuration.
- Move Ticket backend provider/root, Ticket record language, orchestration defaults, and fixed role launch config under a Workspace settings namespace.
- Keep local-only backend/runtime settings separate from tracked project/workspace Ticket policy.
- Update Ticket CLI, Worker Ticket feature setup, workspace-server Ticket backend endpoint, Panel/role launch code, and tests to resolve Ticket settings from the Workspace settings authority.
- Preserve a narrow read-only migration path from `.yoi/ticket.config.toml` only if needed for transition, but do not keep it as the long-term active authority.
- Ensure workspace-server Ticket backend endpoint honors the configured Ticket backend root instead of hard-coding `.yoi/tickets`.

Acceptance criteria:
- New/updated tests cover Workspace settings loading for Ticket backend root, language, orchestration defaults, and role slots.
- Existing Ticket CLI and Worker Ticket tools continue to work against the same `.yoi/tickets` data by default.
- A non-default Ticket backend root configured in Workspace settings is respected by both direct CLI/local paths and workspace-server-backed Worker Ticket tools.
- Documentation or in-code migration notes state that `.yoi/ticket.config.toml` is obsolete/replaced.
- `cargo test` for affected crates and `nix build .#yoi` pass.


---

<!-- event: intake_summary author: hare at: 2026-07-15T16:18:40Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-15T16:18:40Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-15T16:19:31Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-15T16:22:50Z -->

## Decision

Routing decision: implementation_ready_waiting_explicit_followup

Reason:
- Ticket は Workspace project settings を Ticket configuration の durable authority にする具体的な移行作業であり、要件・受け入れ条件・migration/fallback 方針が実装可能な粒度で記録されている。
- `TicketRelationQuery(00001KXK9507N)` は 0 件で、blocking relation はない。
- `TicketOrchestrationPlanQuery(00001KXK9507N)` は事前 record なし。今回 accepted plan と waiting-capacity note を記録した。
- `TicketList(inprogress)` は 0 件、`TicketList(queued)` はこの Ticket 1 件のみ。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` は clean。
- ただし今回の workflow invocation は「explicit follow-up before spawning role Pods」と明示しているため、この routing pass では `queued -> inprogress`、worktree 作成、Coder/Reviewer Pod spawn は行わない。

Evidence checked:
- Ticket body / thread / artifacts。
- `.yoi/workspace.toml` and `.yoi/ticket.config.toml` current files。
- `docs/development/work-items.md` and `docs/design/workflows-public-dogfood-split.md` Ticket config references。
- Code map via grep:
  - `crates/yoi/src/ticket_cli.rs`, `crates/yoi/src/objective_cli.rs`
  - `crates/tui/src/workspace_panel.rs`
  - `crates/worker/src/feature/builtin/ticket.rs`, `crates/worker/src/controller.rs`
  - workspace identity/settings references in `crates/worker`, `crates/worker-runtime`, workspace-server related code。
- Current worktrees/branches: no existing `work/00001KXK9507N-*` branch/worktree。
- Visible Pods: no active role Pod needed for this routing-only pass。

IntentPacket:

Intent:
- `.yoi/ticket.config.toml` を長期 active authority から外し、tracked Workspace project settings の namespace に Ticket configuration を統合する。
- Ticket backend provider/root、Ticket record language、orchestration defaults、固定 role launch config を Workspace settings authority から解決する。

Binding decisions / invariants:
- local-only backend/runtime operational settings（DB path、listen address、runtime endpoint/store 等）と tracked project/workspace Ticket policy を混ぜない。
- Workspace settings が Ticket configuration の durable authority になる。
- `.yoi/ticket.config.toml` は必要な場合だけ narrow read-only migration fallback として扱い、長期 active authority として残さない。
- default behavior は保つ: 設定なし/移行後 default では既存 `.yoi/tickets` data に対して CLI と Worker Ticket tools が動く。
- workspace-server Ticket backend endpoint は `.yoi/tickets` hard-code ではなく configured Ticket backend root を尊重する。
- role launch config は fixed role slots を維持し、Profile/workflow selector の authority を Workspace settings 側に移す。
- Ticket record language policy は durable Ticket records に適用し、通常 worker prose language と混同しない。

Requirements / acceptance criteria:
- Workspace settings loading が Ticket backend root/provider、language、orchestration defaults、role slots を読む test を追加/更新する。
- Ticket CLI / Objective CLI / Worker Ticket feature setup / workspace-server Ticket backend endpoint / Panel role launch path が Workspace settings authority から Ticket settings を解決する。
- Workspace settings で non-default Ticket backend root を設定した場合、direct CLI/local path と workspace-server-backed Worker Ticket tools の両方が尊重する。
- `.yoi/ticket.config.toml` obsolete/replaced の docs または migration/in-code note を残す。
- affected crates の cargo tests と `nix build .#yoi` が通る。

Implementation latitude:
- 既存 `.yoi/workspace.toml` を拡張するか、tracked Workspace project settings file を別に導入するかは、既存 Workspace settings architecture に合わせて coder が選んでよい。ただし local-only backend/runtime settings とは分離すること。
- Legacy `.yoi/ticket.config.toml` fallback の exact scope（read-only import / warning / migration helper）は bounded local tactic としてよいが、active authority として二重管理しないこと。
- DTO/schema/test layout は existing workspace/ticket config patterns に合わせてよい。

Escalate if:
- Workspace project settings の tracked file 形式自体を新規 product decision として固定する必要があり、既存 `.yoi/workspace.toml` / settings architecture から自然に選べない場合。
- `.yoi/ticket.config.toml` と Workspace settings の二重 authority を長期互換として残す必要が出る場合。
- remote/backend Ticket authority の設計が、local path migration を越えて protocol/API redesign を要求する場合。
- existing Ticket storage migration で data loss risk が出る場合。

Validation:
- `git diff --check`
- `cargo test -p ticket` if ticket config crate/tests are touched。
- `cargo test -p yoi --lib --tests` or targeted CLI tests if CLI paths are touched。
- `cargo test -p worker --lib --tests` if Worker Ticket feature/controller paths are touched。
- `cargo test -p yoi-workspace-server --lib` if workspace-server Ticket endpoint/settings are touched。
- `cargo test -p tui --lib --tests` if Panel role launch/config availability is touched。
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test` if web/API types are touched。
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map / likely touch points:
- `crates/ticket` config model/loading。
- `crates/yoi/src/ticket_cli.rs`, `crates/yoi/src/objective_cli.rs`。
- `crates/tui/src/workspace_panel.rs` for Panel config availability/role launch。
- `crates/worker/src/feature/builtin/ticket.rs`, `crates/worker/src/controller.rs` for Worker Ticket tools/setup。
- workspace-server Ticket backend endpoint/settings code。
- `.yoi/workspace.toml` / tracked Workspace settings file and docs under `docs/development/work-items.md`。

Critical risks / reviewer focus:
- accidentally keeping `.yoi/ticket.config.toml` as co-equal active authority。
- mixing local-only backend/runtime settings into tracked project settings。
- direct CLI and workspace-server-backed Worker Ticket tools resolving different Ticket roots/languages。
- role launch config losing fixed role semantics or using `inherit` where top-level role launch cannot support it。
- hard-coded `.yoi/tickets` remaining in workspace-server endpoint or Worker Ticket tools。

Next action:
- Wait for explicit follow-up. On follow-up, re-check Ticket/relation/plan/worktree state, then record `queued -> inprogress` before creating `/home/hare/Projects/yoi/.worktree/00001KXK9507N-ticket-workspace-settings` and spawning role Pods。

---
