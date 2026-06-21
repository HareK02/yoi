<!-- event: create author: "yoi ticket" at: 2026-06-21T06:57:06Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-21T06:58:09Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T06:58:09Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-21T07:11:58Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-21T07:13:24Z -->

## Decision

Routing decision: `implementation_ready`

Reason:
- Ticket body は Workspace web control plane の bootstrap slice として backend crate、store abstraction + SQLite、static SPA skeleton、initial read API、static serving、packaging/Nix hygiene、validation criteria まで具体化されている。
- Objective `00001KVJPT2PP` は Web frontend を primary team UI とし、control plane / runner architecture を段階実装する背景として整合している。
- Relations / orchestration plan に blocker はない。
- Current queued Ticket はこの Ticket のみ。
- Orchestrator worktree は clean on `orchestration` at `5fa0846d` で、対象 Ticket 用 worktree / branch は未作成。
- Visible Pods に対象 Ticket の child Pod は存在しない。

Evidence checked:
- Ticket body / thread via direct read。
- Objective `00001KVJPT2PP` via direct read。
- `TicketRelationQuery(00001KVMFFYVX)`: no relations / blockers。
- `TicketOrchestrationPlanQuery(00001KVMFFYVX)`: no records。
- `TicketList(state=queued)`: queued Ticket はこの Ticket のみ。
- `ListPods`: current visible Pods に対象 Ticket の coder/reviewer はない。
- Orchestrator git state / worktree list / branch list checked from `/home/hare/Projects/yoi/.worktree/orchestration` only。
- Bounded code map:
  - `Cargo.toml` workspace members exist under `crates/*`。
  - Existing project-record / Objective CLI code is in `crates/yoi/src/objective_cli.rs` and Ticket CLI code in `crates/yoi/src/ticket_cli.rs`。
  - No existing frontend package/root was found in active source; frontend skeleton location is an implementation decision。
  - Dependency search found no current web backend crate; adding one is expected。

IntentPacket:

Intent:
- Bootstrap a local single-workspace Workspace web control plane that can serve a static SPA and bounded read APIs while preserving existing `.yoi` Ticket / Objective workflows as canonical project records。
- Establish architecture seams for future multi-workspace hosted control plane, runner connections, event streams, and store implementations without implementing hosted SaaS or runner scheduling in this Ticket。

Binding decisions / invariants:
- Product CLI ownership remains in `yoi` crate; new backend crate must not become the product CLI façade。
- Initial server is single-workspace and local/dev oriented, but internal API/state models should carry `workspace_id` or equivalent to avoid blocking multi-workspace later。
- Store layer has an explicit trait/interface boundary; SQLite is the initial implementation, not an authority leak through frontend or handlers。
- SQLite setup should include server-appropriate basics: WAL, foreign keys, busy timeout, and minimal schema versioning/migration mechanism。
- Existing `.yoi/tickets` and `.yoi/objectives` local record workflow remains canonical and must not be migrated or broken in this Ticket。
- Frontend is static SPA skeleton, not SSR authority and not a place for lifecycle/business authority。
- Rust backend must separate `/api/...` from SPA fallback/static serving。
- Auth can be explicit local-only/dev-token placeholder, but must not imply production SaaS auth is solved。
- No write API, runner dispatch, billing/quota, memory migration, or hosted multi-tenant operations in this Ticket。
- Packaging/Nix/repository hygiene must remain valid; generated build artifacts should not be checked in unless explicitly justified。

Requirements / acceptance criteria:
- Add a workspace control plane backend Rust crate to Cargo workspace。
- Provide HTTP API + static SPA serving surfaces and future extension points for event stream / runner connection。
- Add store abstraction plus initial SQLite implementation with migration/versioning。
- Add bounded initial read APIs at least for workspace, tickets, and objectives; candidate additional empty/skeleton endpoints for runs/runners are allowed if clean。
- Add SvelteKit static SPA skeleton in monorepo and document/encode package manager + lockfile + build artifact handling。
- Backend can serve built static assets and use SPA routing fallback separately from `/api/...`。
- Existing local `.yoi` Ticket / Objective record workflow remains working。
- Validation before completion includes `cargo fmt --check`, relevant `cargo test` / `cargo check`, frontend check/build, `git diff --check`, `yoi ticket doctor`, `yoi objective doctor`, and `nix build .#yoi --no-link`。

Implementation latitude:
- Crate names and paths may be chosen by Coder, with preference for clear names such as `workspace-server` / `workspace` / `control-plane` and avoiding ambiguity with runtime workspace root semantics。
- Static asset embedding/serving may be implemented as fallback directory serving, optional embedded assets, or a documented dev/static-dir path if the initial bootstrap remains runnable and package-safe。
- SQLite crate choice may follow current project style/dependency constraints; dependency/package hash updates must be handled if new dependencies are added。
- Frontend package manager may be npm/pnpm/etc. if lockfile and Nix/package handling are explicit and reproducible enough for this bootstrap。
- API JSON schemas can be minimal and bounded; do not overbuild mutation or runner dispatch。
- Add focused tests around store migration, `.yoi` record read bridge, handler/API shape, and static/API route separation.

Escalate if:
- Adding frontend build tooling cannot be reconciled with Nix/package build in this slice。
- SQLite dependency/package updates create unresolved Nix cargo hash/source-filter failures。
- Serving built SPA assets would require checking in generated artifacts without agreement。
- `.yoi` Ticket/Objective canonical record compatibility becomes ambiguous or requires migration。
- Implementing this bootstrap forces public auth/hosted SaaS decisions beyond local/dev mode。
- The change grows into write API / runner dispatch / scheduler design rather than bootstrap/read-heavy surface。

Validation plan for Coder and Reviewer:
- Rust: `cargo fmt --check`, focused tests for new crate / yoi integration, `cargo check` for affected crates and workspace-facing binary。
- Frontend: install/check/build command appropriate to chosen package manager, with lockfile committed if needed。
- Repository/package: `git diff --check`, `yoi ticket doctor`, `yoi objective doctor`, `nix build .#yoi --no-link` if dependency/package/source-filter/frontend handling changed。
- Smoke: start or exercise server routes in a test/noninteractive way for `/api/workspace`, `/api/tickets`, `/api/objectives`, and static/SPA fallback。

Critical risks / reviewer focus:
- Keep authority in Rust backend/store, not frontend。
- Ensure `/api/...` routes do not fall through to SPA fallback incorrectly。
- Ensure local `.yoi` records remain canonical and existing CLI doctor workflows still pass。
- Ensure SQLite migrations are deterministic and not tied to process cwd accidentally。
- Ensure frontend package files and generated artifacts do not pollute package/Nix builds。
- Ensure new dependencies and Nix cargo hash/source filtering are updated consistently。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-21T07:13:29Z from: queued to: inprogress reason: human_authorized_unblocked_implementation field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete acceptance criteria and no recorded blockers, so Orchestrator accepts implementation and will create a child implementation worktree before spawning sibling Coder/Reviewer roles.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T07:15:03Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVMFFYVX-workspace-web-control-plane`
- Created branch:
  - `impl/00001KVMFFYVX-workspace-web-control-plane`
- Base commit:
  - `1d27f6c9 ticket: accept workspace web control plane`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVMFFYVX`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVMFFYVX-workspace-web-control-plane`

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff. Orchestrator will not merge/close until reviewer approval and validation evidence are available。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-21T07:37:30Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVMFFYVX`.

Implementation commit:
- `ab7658c1 feat: bootstrap workspace web control plane`

Changed areas:
- New Cargo workspace crate:
  - `crates/workspace-server`
  - HTTP router with `/api/...` separation from static/SPA fallback。
  - Local/dev workspace API state carrying `workspace_id`。
  - Bounded read APIs for `/api/workspace`, `/api/tickets`, `/api/tickets/{id}`, `/api/objectives`, `/api/objectives/{id}`, `/api/runs`, `/api/runners`。
  - Reserved extension-point response fields for future event streams and runner connections。
- Store abstraction and SQLite implementation:
  - `ControlPlaneStore` trait。
  - `SqliteWorkspaceStore`。
  - migration/version table。
  - WAL, foreign keys, busy timeout。
  - initial schema for workspace/runtime/projection seams while keeping `.yoi` records canonical。
- Local project-record bridge:
  - reads existing `.yoi/tickets`。
  - reads existing `.yoi/objectives`。
  - does not migrate or mutate canonical records。
- SvelteKit static SPA skeleton:
  - `web/workspace`
  - npm + committed `package-lock.json`
  - static adapter build output to `web/workspace/build`
  - generated artifact handling documented in `web/workspace/README.md`。
- Packaging/source handling:
  - added Rust dependencies and updated `Cargo.lock`。
  - updated `package.nix` cargo hash。
  - excluded generated frontend directories from Nix package source filtering。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `cargo check -p yoi`: passed
- `cd web/workspace && npm run check && npm run build`: passed
- `git diff --check`: passed
- `cargo run -p yoi -- ticket doctor`: passed
- `cargo run -p yoi -- objective doctor`: passed
- `nix build .#yoi --no-link`: passed after updating `cargoHash`

Coder dependency/package/frontend decisions:
- Used `axum` for HTTP routing。
- Used `rusqlite` with bundled SQLite to avoid system SQLite packaging requirements。
- Backend remains a library crate, not a product CLI façade。
- No product `yoi` CLI command was added in this slice。
- Frontend uses npm/SvelteKit static adapter with committed lockfile; generated build output is not checked in。
- Nix package does not build frontend in this bootstrap; it excludes generated frontend artifacts from source filtering。

Known deferrals reported:
- No write API。
- No runner job dispatch or scheduler。
- No hosted/multi-tenant auth or SaaS behavior。
- No event stream implementation yet; only reserved extension-point seam。
- No checked-in generated SPA assets; backend can serve configured static directory such as `web/workspace/build` after running frontend build。

Orchestrator evidence checked:
- Implementation worktree is clean on `impl/00001KVMFFYVX-workspace-web-control-plane`。
- HEAD is `ab7658c1`。
- Diff `1d27f6c9..HEAD` touches expected backend/frontend/package files: 18 files, about 3377 insertions / 1 deletion。
- `git diff --check 1d27f6c9..HEAD` produced no diagnostics。

Next action:
- Route to read-only Reviewer Pod for acceptance review against the Ticket and Objective context。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-21T07:38:14Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVMFFYVX-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVMFFYVX-workspace-web-control-plane`
- Branch: `impl/00001KVMFFYVX-workspace-web-control-plane`
- Baseline: `1d27f6c9`
- Implementation commit: `ab7658c1 feat: bootstrap workspace web control plane`
- Diff: `1d27f6c9..ab7658c1`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Acceptance criteria for backend crate, store abstraction + SQLite, static SPA, bounded read APIs, static/API route separation, package/Nix/frontend hygiene。
- No write API / scheduler / SaaS overreach。
- `.yoi` Ticket / Objective records remain canonical and read-only。
- SQLite migrations/settings are deterministic and server-appropriate。
- Frontend generated artifacts are not committed and source filtering is safe。
- Validation evidence includes Rust tests/checks, frontend check/build, doctors, and `nix build .#yoi --no-link`。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-reviewer-00001KVMFFYVX-r1 at: 2026-06-21T07:44:49Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket/context:
  - `.yoi/tickets/00001KVMFFYVX/item.md`
  - `.yoi/tickets/00001KVMFFYVX/thread.md`
  - `.yoi/objectives/00001KVJPT2PP/item.md`
- Diff `1d27f6c9..ab7658c1`:
  - `Cargo.toml`
  - `Cargo.lock`
  - `crates/workspace-server/Cargo.toml`
  - `crates/workspace-server/src/lib.rs`
  - `crates/workspace-server/src/server.rs`
  - `crates/workspace-server/src/store.rs`
  - `crates/workspace-server/src/records.rs`
  - `package.nix`
  - `web/workspace/package.json`
  - `web/workspace/package-lock.json`
  - `web/workspace/.gitignore`
  - `web/workspace/README.md`
  - `web/workspace/svelte.config.js`
  - `web/workspace/vite.config.ts`
  - `web/workspace/tsconfig.json`
  - `web/workspace/src/app.html`
  - `web/workspace/src/routes/+layout.ts`
  - `web/workspace/src/routes/+page.svelte`

Blocking issues:
- None found。

Acceptance verification:
- New `yoi-workspace-server` crate is a library/backend crate, not a product CLI façade。
- Existing `yoi` CLI ownership is preserved; `yoi` does not depend on the new crate。
- HTTP routes are read-only `GET` routes for `/api/workspace`, `/api/tickets`, `/api/tickets/{id}`, `/api/objectives`, `/api/objectives/{id}`, `/api/runs`, `/api/runners`。
- SPA/static fallback explicitly rejects `/api` and `/api/...`, so API paths are not swallowed by SPA fallback。
- `.yoi/tickets` and `.yoi/objectives` remain canonical read sources; no `.yoi` write/migration path was introduced。
- Store abstraction is explicit via `ControlPlaneStore`; SQLite is behind `SqliteWorkspaceStore`。
- SQLite setup includes migrations/version table, WAL, foreign keys, and busy timeout。
- Runtime tables for runs/runners are skeletal/read-only and do not implement scheduling or dispatch。
- Auth is explicitly represented as local/dev placeholder, not production SaaS auth。
- SvelteKit frontend is static SPA skeleton with npm lockfile; generated `node_modules`, `.svelte-kit`, and `build` are ignored/excluded。
- `package.nix` source filtering excludes frontend generated artifacts; `nix build .#yoi --no-link` passed with updated cargo hash。

Non-blocking concerns:
- No committed CLI/binary entrypoint or full listener smoke test for starting the server end-to-end. Library exposes `serve(...)` and router tests cover API/static separation, acceptable for bootstrap; follow-up should add launch surface when product command is designed。
- Detail responses for tickets/objectives do not include `workspace_id`, while list/runtime responses do. Future multi-workspace API expansion should make detail shapes consistently workspace-qualified。
- Missing valid objective IDs appear likely to map filesystem `Io` errors to HTTP 500 rather than 404. Tighten when read API contract is hardened。
- Static path traversal rejects `..`, but symlink escape hardening is not implemented. Acceptable for configured local build/static directory and local/dev bootstrap, revisit before non-local exposure。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check 1d27f6c9..HEAD`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `cargo check -p yoi`: passed
- `cd web/workspace && npm run check && npm run build`: initially failed because `node_modules` absent。
- `cd web/workspace && npm ci && npm run check && npm run build`: passed
- `cargo run -p yoi -- ticket doctor`: passed
- `cargo run -p yoi -- objective doctor`: passed
- `nix build .#yoi --no-link`: passed
- Focused route/static validation:
  - Inspected router definitions and fallback logic。
  - Existing `yoi-workspace-server` tests exercise `/api/workspace`, `/api/tickets`, `/api/objectives`, `/api/runners`, static asset serving, SPA fallback, and `/api/...` miss separation。
  - Store migration/persistence and local record read bridge are covered by crate tests。

Worktree status:
- Tracked worktree clean at `ab7658c1` on `impl/00001KVMFFYVX-workspace-web-control-plane`。
- Ignored validation artifacts remain in the implementation worktree: `web/workspace/node_modules/`, `web/workspace/.svelte-kit/`, `web/workspace/build/`。

---
