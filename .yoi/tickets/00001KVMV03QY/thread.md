<!-- event: create author: ticket-intake at: 2026-06-21T10:18:10Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T10:56:31Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T10:57:43Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket body は `web/workspace` の frontend tooling を npm/Node から Deno に移行する concrete follow-up として、requirements / acceptance criteria / validation が具体化されている。
- `readiness: implementation_ready` で、relations / orchestration plan に blocker はない。
- `00001KVMFFYVX` Workspace web control plane bootstrap は closed 済みで、移行対象の `web/workspace` が現在の `orchestration` に存在する。
- `00001KVMT2J25` は protocol reconnect Ticket として別件であり、この Ticket の scope には混ぜないことが body に明記されている。
- 同時 queued の `00001KVMT2J25` は protocol/pod/TUI stream-state work で、主対象が異なるため並列実装可能と判断する。
- Orchestrator worktree は clean on `orchestration` at `b4786b40` で、対象 Ticket 用 worktree / branch は未作成。

Evidence checked:
- Ticket body / thread / artifacts via `TicketShow` and direct `item.md` read。
- `TicketRelationQuery(00001KVMV03QY)`: no relations / blockers。
- `TicketOrchestrationPlanQuery(00001KVMV03QY)`: no records。
- Orchestrator git state / worktree list / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。
- Bounded code map:
  - `web/workspace/package.json`, `package-lock.json`, `README.md`, `svelte.config.js`, `vite.config.ts`, `tsconfig.json`, `.gitignore` are current frontend tooling files。
  - `package.nix` currently excludes `web/workspace/node_modules`, `.svelte-kit`, and `build`。
  - `devshell.nix` already includes `deno`。

IntentPacket:

Intent:
- Move Workspace web SPA frontend tooling from npm/Node-primary to Deno-primary while keeping SvelteKit static SPA and Rust backend authority unchanged。

Binding decisions / invariants:
- This is frontend tooling migration only; do not change Workspace backend API authority, Ticket/Objectives authority, or `.yoi` canonical record workflows。
- Keep static SPA and Rust backend static-serving boundary。
- Do not turn frontend into SSR/business/lifecycle authority。
- Avoid ambiguous dual source-of-truth between npm and Deno。
- Generated artifacts must remain out of git and Nix package source。
- `00001KVMT2J25` protocol reconnect work remains separate。

Requirements / acceptance criteria:
- `web/workspace` check/build succeeds through Deno commands。
- Add/clarify Deno project config such as `deno.json`/`deno.lock`/`deno task`。
- README and project files make normal Deno install/check/build workflow clear。
- Remove npm lockfile/scripts/Node-only guidance if unnecessary; if any npm/Node metadata remains, document whether it is compatibility artifact or canonical source-of-truth。
- Preserve static output usable by Rust backend, default `web/workspace/build/` unless deliberately changed and documented。
- Keep `web/workspace/build/`, `.svelte-kit/`, `node_modules` or Deno equivalents ignored/source-filtered。
- Update `package.nix` / `devshell` / README only as needed for generated artifacts and validation commands。

Implementation latitude:
- Use Deno npm compatibility for SvelteKit/Vite/svelte-check if practical。
- Keep `tsconfig.json` if Svelte LSP / svelte-check needs it。
- Keep `@sveltejs/adapter-static` if static backend serving remains compatible。
- Minimal `package.json` can remain if ecosystem requires it, but source-of-truth must be clear。

Escalate if:
- Deno-only check/build is unstable and Node/npm must remain primary。
- Keeping/removing `package.json` or `package-lock.json` becomes a policy decision rather than implementation detail。
- Migration requires SSR/Deno runtime server/Deno Deploy assumptions。
- Rust backend static serving path or Nix package strategy requires major redesign。
- Generated artifact boundaries become unclear。

Validation plan:
- `cd web/workspace && deno task check`
- `cd web/workspace && deno task build`
- `git diff --check`
- generated artifact ignore/source-filter check。
- `nix build .#yoi --no-link` if package/source-filter changes。
- `cargo check -p yoi-workspace-server` or focused server/static tests if static serving assumptions change。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T10:57:48Z from: queued to: inprogress reason: human_authorized_unblocked_frontend_tooling_migration field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete acceptance criteria, no recorded blockers, and is semantically separate from the protocol reconnect Ticket, so Orchestrator accepts implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T11:00:00Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVMV03QY-workspace-spa-deno`
- Created branch:
  - `impl/00001KVMV03QY-workspace-spa-deno`
- Base commit:
  - `155e039e ticket: route deno and inflight snapshot work`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVMV03QY`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVMV03QY-workspace-spa-deno`

Parallelization note:
- `00001KVMT2J25` is active in a separate child worktree and targets protocol/pod/TUI stream state. This Ticket should stay limited to `web/workspace` frontend tooling and package/source-filter docs as needed。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---
