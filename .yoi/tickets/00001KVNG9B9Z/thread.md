<!-- event: create author: "yoi ticket" at: 2026-06-21T16:30:12Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-21T16:30:49Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T16:30:49Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T16:35:06Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T16:36:30Z -->

## Decision

Routing decision: `implementation_ready_parallel_with_conflict_risk`

Reason:
- Ticket body は Workspace web UI に左 sidebar navigation panel を追加する UX skeleton として、header / repositories / objectives / workers sections、responsive behavior、section-level error handling、Deno validation まで具体化されている。
- Relations are `related` only to `00001KVMFFYVX` and `00001KVNEKH9Q`; no blocking dependency is recorded。
- `00001KVMFFYVX` is closed/integrated。
- `00001KVNEKH9Q` is currently in review and touches Host/Worker API plus `web/workspace/src/routes/+page.svelte`; this creates merge-conflict risk but not an authority blocker. Ticket body explicitly allows Workers section to be placeholder before Host/Worker API exists and asks for component boundary that can later connect to that API。
- Current queued Ticket is this Ticket only。
- Orchestrator worktree is clean on `orchestration` at `d4de8e26`; target worktree / branch is not present。

Evidence checked:
- Ticket body via direct `item.md` read。
- `relations.json`: related to `00001KVMFFYVX` and `00001KVNEKH9Q` only。
- `TicketOrchestrationPlanQuery(00001KVNG9B9Z)`: no records。
- `TicketList(state=queued)`: this Ticket is the only queued Ticket。
- `00001KVNEKH9Q` current state is `inprogress` / reviewer running; no approval/merge yet。
- Orchestrator git state / worktree list / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。

IntentPacket:

Intent:
- Add a left sidebar navigation skeleton to Workspace web SPA that surfaces Workspace / Repository / Objective / Worker navigation without adding deep editing or business authority。

Binding decisions / invariants:
- Frontend-only UI skeleton unless a minimal API read is already available; do not change backend authority without need。
- Keep static SPA; no SSR or frontend lifecycle/business authority。
- Objective section should read existing `/api/objectives` and display bounded title/state data。
- Worker section should use a component/data boundary that can connect to Host/Worker API, but must safely show placeholder/empty/error if API is absent or in flux。
- Do not expose Pod as primary UI/domain naming; use `workers`。
- API failures are section-local and must not take down the whole page。
- Avoid broad design-system churn; keep current skeleton style direction。
- Be aware that `00001KVNEKH9Q` may merge Host/Worker UI/API changes concurrently; keep sidebar changes narrow and componentized to reduce conflict。

Requirements / acceptance criteria:
- Left sidebar is visible in Workspace web UI。
- Sidebar header displays workspace name/label plus settings placeholder/icon/button。
- Sections present: `repositories`, `objectives`, `workers`。
- Objectives section fetches existing `/api/objectives` and displays title/state minimal info。
- Workers section safely displays placeholder/empty/error if Host/Worker API is not yet integrated; if API is available in branch, it may use it through a component boundary。
- Main content and sidebar layout are separated and responsive enough to avoid narrow-viewport breakage / horizontal overflow。
- Section-level error/empty states are bounded。
- Deno check/build passes。

Implementation latitude:
- Split Svelte components under `web/workspace/src/lib` if useful。
- Keep repository section placeholder/local summary if no Repository API exists。
- Use simple CSS layout; no full design system。
- If concurrent `00001KVNEKH9Q` merge creates conflict, report rather than broad rewrite。

Escalate if:
- Implementing sidebar requires backend API redesign beyond minimal frontend skeleton。
- Workers section cannot be safely decoupled from unmerged Host/Worker API。
- Deno/Svelte tooling fails for reasons unrelated to this Ticket。
- Merge conflict with `00001KVNEKH9Q` becomes non-trivial and requires orchestration sequencing。

Validation plan:
- `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link` preferred because Workspace frontend package surface is touched。
- `cargo test -p yoi-workspace-server` only if backend code changes。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T16:36:37Z from: queued to: inprogress reason: human_authorized_unblocked_sidebar_ui_implementation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete frontend acceptance criteria and no blocking relation; related Host/Worker API work is in review and creates merge-conflict risk but not an authority blocker. Orchestrator accepts implementation with narrow frontend scope.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T16:37:34Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVNG9B9Z-workspace-sidebar`
- Created branch:
  - `impl/00001KVNG9B9Z-workspace-sidebar`
- Base commit:
  - `2c7ef24a ticket: accept workspace sidebar ui`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVNG9B9Z`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVNG9B9Z-workspace-sidebar`

Parallelization note:
- `00001KVNEKH9Q` remains active in review and may merge Host/Worker API plus `web/workspace/src/routes/+page.svelte` changes. Coder was instructed to keep sidebar work narrow/componentized and report if conflicts become non-trivial。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---
