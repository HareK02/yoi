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
