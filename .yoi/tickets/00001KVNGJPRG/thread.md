<!-- event: create author: "yoi ticket" at: 2026-06-21T16:35:19Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-21T16:36:06Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T16:36:06Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T16:40:35Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T16:41:09Z -->

## Decision

Routing decision: `wait_do_not_parallelize_with_sidebar`

Reason:
- Ticket body is implementation-ready in isolation, but it is directly coupled to the currently active sidebar navigation work `00001KVNG9B9Z`。
- Relations show this Ticket is related to `00001KVNG9B9Z` with note: “Sidebar navigation should link to repository and objective pages”。
- `00001KVNG9B9Z` is already `inprogress`, has a live Coder Pod, and is expected to change `web/workspace/src/routes/+page.svelte` / sidebar component structure。
- Starting this Ticket in parallel now would likely produce overlapping route/layout/sidebar changes and non-trivial semantic merge work, rather than independent implementation。
- Host/Worker API work `00001KVNEKH9Q` just landed, further increasing the need to base the page/navigation work on current SPA state。

Decision:
- Do not create a worktree or spawn Coder for `00001KVNGJPRG` yet。
- Leave Ticket `queued` and record an orchestration `do_not_parallelize` note with `00001KVNG9B9Z`。
- Re-route this Ticket after sidebar work lands, or if sidebar implementation report shows a stable component boundary that makes repository/objective pages non-conflicting。

No implementation side effects were performed for this Ticket。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T17:02:35Z -->

## Decision

Routing decision: `implementation_ready_after_sidebar_landed`

Reason:
- This Ticket was previously left queued due `do_not_parallelize` with `00001KVNG9B9Z` because repository/objective pages and sidebar navigation both touched the Workspace SPA navigation/layout surface。
- `00001KVNG9B9Z` is now `closed` and integrated (`613f4126 merge: workspace sidebar navigation`), so the sequencing blocker is cleared。
- Related bootstrap Ticket `00001KVMFFYVX` is also closed/integrated。
- Ticket body has concrete backend/API/frontend requirements, acceptance criteria, and validation requirements。
- Relations are `related` only; no blocking relation remains。
- Orchestrator worktree is clean on `orchestration` at `eb2e5907`; target worktree / branch is not present。

IntentPacket:

Intent:
- Add Repository and Objective pages reachable from the Workspace sidebar, using filesystem read-through Ticket/Objective authority and bounded read-only Repository/Git summaries。

Binding decisions / invariants:
- Ticket / Objective canonical authority remains existing filesystem records; do not migrate canonical writes to SQLite or add mutation APIs。
- Repository page is read-only initial slice for current workspace root as local Repository。
- Git info and log summaries must be bounded; do not expose full diffs, file contents, blame, or secret-like config。
- Repository Ticket Kanban is read-only and grouped by Ticket state; no drag/drop or state mutation。
- Objective list uses existing filesystem read-through `/api/objectives` data, with detail links/placeholders as practical。
- Frontend remains static SPA; no SSR/business authority。
- Sidebar links should use the now-landed navigation/component structure。
- API failures, non-Git repo, and empty state must be section-local。

Requirements / acceptance criteria:
- Add read-only Repository list/detail/summary API or minimal workspace repository summary。
- For Git repository: bounded branch/head/root/dirty/remote/recent log summary。
- For non-Git repository: safe `kind = local` / git unavailable diagnostic。
- Add bounded Git log summary API returning recent N commit hash/subject/author/timestamp only。
- Add Repository Ticket Kanban read model grouped by Ticket state, with safe fallback to workspace-local tickets when target metadata is absent。
- Add Repository page showing summary, Git summary/log, and Ticket Kanban。
- Add Objective list page showing objective title/state/updated_at/summary and detail link/placeholder if feasible。
- Sidebar repositories/objectives links navigate to these pages。
- Deno check/build and backend validation pass。

Implementation latitude:
- Use SvelteKit static SPA client-side routing/hash/router approach or simple stateful navigation if current skeleton lacks route generation; keep static output compatible。
- Backend API names may be pragmatic, e.g. `/api/repositories`, `/api/repositories/{id}`, `/api/repositories/{id}/log`, `/api/repositories/{id}/tickets`。
- Current workspace root can be the only repository for this slice。
- Ticket Kanban fallback can group all workspace tickets by state with diagnostic that target metadata is not yet available。
- Keep UI simple and avoid broad design-system churn。

Escalate if:
- Implementing repository APIs would require reading file contents/diffs or unsafe Git config/secrets。
- Objective detail requires mutation or schema changes。
- Static SPA route approach conflicts with SvelteKit adapter/static constraints。
- Sidebar/navigation architecture from `00001KVNG9B9Z` cannot support pages without broad rewrite。

Validation plan:
- `cargo fmt --check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link`。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T17:02:42Z from: queued to: inprogress reason: sidebar_dependency_landed_unblocked_implementation field: state -->

## State changed

Previously recorded `do_not_parallelize` sequencing reason is cleared because sidebar navigation Ticket `00001KVNG9B9Z` is closed and integrated. Human queue authorization remains applicable, and the Ticket has concrete implementation requirements, so Orchestrator accepts implementation.

---
