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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T11:05:28Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVMV03QY`.

Implementation commit:
- `66f04e04 feat: migrate workspace spa tooling to deno`

Changed areas:
- Added Deno-primary frontend tooling:
  - `web/workspace/deno.json`
  - `web/workspace/deno.lock`
- Removed npm lockfile:
  - deleted `web/workspace/package-lock.json`
- Reduced `web/workspace/package.json` to minimal SvelteKit/Vite ecosystem metadata only:
  - no scripts
  - no dependencies
  - `deno.json` / `deno.lock` are canonical source of truth
- Updated `web/workspace/README.md` with Deno workflow:
  - `deno install`
  - `deno task check`
  - `deno task build`
  - npm compatibility usage and generated artifact boundaries
- Preserved static SPA output path:
  - `web/workspace/build/`

Coder validation reported:
- `cd web/workspace && deno task check`: passed
- `cd web/workspace && deno task build`: passed
- `cd web/workspace && deno task install`: passed
- `git diff --check`: passed
- `target/debug/yoi ticket doctor`: passed (`doctor: ok`)
- Generated artifact ignore/source-filter check:
  - `web/workspace/node_modules`, `.svelte-kit`, and `build` are ignored by `web/workspace/.gitignore`
  - existing `package.nix` source filter already excludes those same trees

Not run by Coder:
- `nix build .#yoi --no-link`: not run because `package.nix` / source filter were not changed。
- `cargo check -p yoi-workspace-server`: not run because Rust backend/static-serving assumptions and code were not changed。

Generated artifacts cleanup:
- Removed `web/workspace/node_modules/`, `.svelte-kit/`, and `build/` after validation。

Orchestrator evidence checked:
- Implementation worktree is clean on `impl/00001KVMV03QY-workspace-spa-deno`。
- HEAD is `66f04e04`。
- Diff `155e039e..HEAD` touches expected frontend tooling files only: `README.md`, `deno.json`, `deno.lock`, `package.json`, and deleted `package-lock.json`。
- `git diff --check 155e039e..HEAD` produced no diagnostics。
- `git status --ignored --short web/workspace` showed no remaining generated frontend artifacts。

Next action:
- Route to read-only Reviewer Pod for acceptance review against the Ticket。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-21T11:06:03Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVMV03QY-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVMV03QY-workspace-spa-deno`
- Branch: `impl/00001KVMV03QY-workspace-spa-deno`
- Baseline: `155e039e`
- Implementation commit: `66f04e04 feat: migrate workspace spa tooling to deno`
- Diff: `155e039e..66f04e04`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Deno task check/build works from clean generated-artifact state。
- `deno.json` / `deno.lock` are canonical source of truth。
- `package.json` remains only compatibility metadata if retained; no npm scripts / Node-only guidance remains active。
- Static output path and Rust backend serving assumptions remain valid。
- Generated artifacts remain ignored/source-filtered and are not committed。
- No protocol/backend authority scope creep。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-reviewer-00001KVMV03QY-r1 at: 2026-06-21T11:12:52Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket authority:
  - `.yoi/tickets/00001KVMV03QY/item.md`
  - `.yoi/tickets/00001KVMV03QY/thread.md`
- Diff `155e039e..66f04e04`:
  - `web/workspace/README.md`
  - `web/workspace/deno.json`
  - `web/workspace/deno.lock`
  - `web/workspace/package.json`
  - deleted `web/workspace/package-lock.json`
- Boundary / generated artifact files:
  - `web/workspace/.gitignore`
  - `web/workspace/svelte.config.js`
  - `web/workspace/vite.config.ts`
  - `web/workspace/tsconfig.json`
  - `web/workspace/src/routes/+layout.ts`
  - `package.nix`
  - `devshell.nix`

Blocking issues:
- None。

Findings:
- Deno is now the clear frontend tooling source of truth:
  - `web/workspace/deno.json` defines Deno tasks and pinned npm-compat dependencies。
  - `web/workspace/deno.lock` is committed。
  - `package-lock.json` is removed。
  - `package.json` is reduced to package metadata only, with no scripts or dependencies。
  - README explicitly says canonical tooling is `deno.json` / `deno.lock`, and `package.json` is compatibility metadata only。
- Static SPA assumptions are preserved:
  - `svelte.config.js` still uses `@sveltejs/adapter-static`。
  - output remains `web/workspace/build/`。
  - `+layout.ts` still has `ssr = false` and `prerender = true`。
  - diff does not touch Rust backend/API authority, protocol reconnect scope, or `.yoi` canonical records。
- Generated artifact boundaries remain intact:
  - `.gitignore` ignores `node_modules`, `.svelte-kit`, and `build`。
  - `package.nix` still excludes `web/workspace/node_modules`, `web/workspace/.svelte-kit`, and `web/workspace/build`。
  - `devshell.nix` already provides `deno`。

Non-blocking concern:
- `deno task build` succeeds but upstream SvelteKit/Vite emits generic `Run npm run preview to preview your production build locally.` The project README and `deno.json` correctly provide `deno task preview`, and there are no npm scripts in the repo, so this is not blocking。

Reviewer validation:
- `git diff --check 155e039e..66f04e04`: passed
- `cd web/workspace && deno --version && rm -rf node_modules .svelte-kit build && deno task check && deno task build`: passed with Deno `2.7.14`; `svelte-check found 0 errors and 0 warnings`; build completed to `build`
- `cd web/workspace && deno task install`: passed
- `target/debug/yoi ticket doctor`: passed (`doctor: ok`)
- `nix build .#yoi --no-link`: passed
- `git show --no-patch --format='%H %s' 66f04e04` and `git diff --quiet 66f04e04..HEAD`: HEAD matches implementation commit

Generated artifacts:
- Reviewer did not clean generated ignored artifacts after validation。
- Ignored artifacts present in implementation worktree:
  - `web/workspace/node_modules/`
  - `web/workspace/.svelte-kit/`
  - `web/workspace/build/`
- `git check-ignore -v` confirms all three are ignored by `web/workspace/.gitignore`; `package.nix` excludes same paths。

Worktree status:
- Tracked status clean at `66f04e04` on `impl/00001KVMV03QY-workspace-spa-deno`。

---
