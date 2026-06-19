<!-- event: create author: "yoi ticket" at: 2026-06-19T13:18:58Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-19T13:34:43Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-19T16:21:31Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body には、Component Model runtime path の intent、requirements、acceptance criteria、non-goals、implementation notes、validation が実装可能な粒度で揃っている。
- depends_on の `00001KV5W3PHW` minimal WASM runtime と `00001KV5W3PJ3` permission grant enforcement は closed。
- Related/context work はすべて完了または non-blocking context として確認した。
  - `00001KVFD3YSV` Plugin CLI inspection: closed。
  - `00001KVFDX9AF` HTTPS host API: closed。
  - `00001KVFDX9AY` FS host API: closed。
  - `00001KSXRQ4G8` is planning design context, not blocking relation authority。
- Prior waiting-capacity notes の blocker は解消した。現在 inprogress Ticket は 0 件、child implementation Pod はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は component-model / WIT / runtime-backend / sandbox / packaging / SDK だが、Ticket は existing raw core-Wasm packages を silently reinterpret しない、grants before Tool registration/execution/host API access、no ambient WASI fs/network/env、ordinary Tool history path、runtime kind selected by manifest metadata などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVG0HR96` body / thread / artifacts。
- `TicketRelationQuery(00001KVG0HR96)`: depends_on blockers は closed。related records は context link。
- `TicketOrchestrationPlanQuery(00001KVG0HR96)`: previous waiting notes were based on active CLI/HTTPS/FS work; all are now closed. 今回 `accepted_plan` を記録済み。
- Current workspace state:
  - Orchestrator worktree clean。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
- Code/docs context:
  - `crates/manifest/src/plugin.rs`: current runtime metadata and `yoi-plugin-wasm-1` validation。
  - `crates/pod/src/feature/plugin.rs`: current core-Wasm Plugin runtime, Tool registration/static inspection, HTTPS/FS host APIs, import validation。
  - `crates/yoi/src/plugin_cli.rs`: inspection output should report Component runtime metadata without execution。
  - Ticket body references `docs/design/plugin-component-model.md`, `docs/design/plugin-packages.md`, and Objective `00001KVG0HR9M` as design context.

IntentPacket:

Intent:
- Add explicit WebAssembly Component Model runtime support for Plugin Tool packages while preserving existing Plugin discovery, enablement, digest pinning, ToolRegistry integration, ordinary Tool history, and Plugin grant enforcement.
- Move Plugin authoring/runtime path toward WIT/canonical ABI so future `https`, `fs`, SDK, Service/Ingress APIs do not entrench the raw pointer/length core-Wasm ABI.

Binding decisions / invariants:
- Existing raw core-Wasm packages must not be silently reinterpreted as components。
- Runtime selection is manifest-driven. Component packages use explicit runtime metadata such as `kind = "wasm-component"`, component artifact path, and expected world。
- Existing raw runtime remains explicit (`kind = "wasm"`, `abi = "yoi-plugin-wasm-1"`) unless a migration/deprecation decision is recorded in this Ticket with tests updated accordingly。
- Package discovery and inspection remain read-only and must not execute components。
- Explicit enablement and digest/version/source pinning remain authoritative。
- Plugin grants are checked before Tool registration/execution and before host API calls。
- WIT imports are not authority by themselves。
- No ambient WASI filesystem/network/env is exposed。
- Component Tool registration still goes through existing ToolRegistry / model-visible schema path。
- Tool calls/results use ordinary Worker/Tool history path; no hidden context injection。
- HTTPS/FS host API security boundaries already implemented must be preserved。

Requirements / acceptance criteria:
- A package with `runtime.kind = "wasm-component"` and expected WIT world can be discovered, enabled, registered as a Tool, and executed。
- Sample Component Model Tool Plugin returns a normal Tool result through ordinary Tool path。
- Sample Plugin author source uses generated/SDK bindings rather than raw pointer/length imports/exports。
- Component Tool execution is denied without matching Plugin grants。
- Component host imports cannot bypass Plugin grant model。
- Wrong world / missing export / incompatible component fails closed with bounded diagnostic。
- Existing raw core-Wasm runtime remains explicitly supported, or a migration/deprecation decision is recorded and tests updated。
- `yoi plugin list/show` reports Component runtime metadata without executing components。
- Documentation is updated with authoring/runtime instructions and migration notes。
- Build/package impact is measured and Nix packaging/cargo hash updated if dependencies change。

Implementation latitude:
- Use `wasmtime::component` / WIT tooling or another narrow backend consistent with the codebase。
- Choose WIT names that version cleanly, e.g. `yoi:plugin/tool@1.0.0` and `yoi:host/https@1.0.0` / `yoi:host/fs@1.0.0`。
- If a staged approach is unavoidable, escalate before narrowing completion. Do not land manifest parsing alone as if it completes this Ticket。
- Keep compatibility layer and Component runtime dispatch cleanly separated。
- Use focused sample fixtures/tests rather than broad E2E process spawning。

Escalate if:
- Component runtime execution cannot be implemented without a broad architecture redesign。
- Dependency/build-size impact is large enough to need product decision。
- WIT/tool request-response typing requires a product/API decision beyond Ticket latitude。
- Preserving both raw core-Wasm and Component runtime would substantially distort implementation。
- SDK/sample generation requires external toolchain not feasible in repository validation。

Validation:
- Focused Component Plugin manifest/discovery/static inspection tests。
- Component Tool registration and execution tests。
- Grant denial before Component Tool execution / host API access。
- Wrong world / missing export / incompatible component fail-closed tests。
- Existing raw core-Wasm Plugin runtime tests remain passing or migration decision/tests updated。
- `cargo fmt --check`。
- `git diff --check`。
- relevant `cargo check` / `cargo test`。
- `nix build .#yoi` because component runtime dependencies / packaging are likely to change。

Critical risks / reviewer focus:
- WIT imports becoming implicit authority。
- Component runtime bypassing existing Plugin grant enforcement。
- Ambient WASI fs/network/env exposure。
- Component execution bypassing ordinary Tool result/history path。
- Breaking existing raw core-Wasm package behavior without explicit decision/tests。
- Inspection accidentally executing components。
- Unbounded or secret-leaking diagnostics。
- Packaging/Nix/Cargo dependency correctness and binary/build-time impact。

Next action:
- `queued -> inprogress` を記録し、Ticket records を Orchestrator worktree に commit してから、専用 implementation worktree を作成し Coder Pod を narrow write scope で起動する。root/original workspace は操作しない。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-19T16:21:50Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_component_model_runtime field: state -->

## State changed

Ticket body/thread, relation metadata, orchestration plan records, related completed Tickets, Orchestrator worktree, visible Pods, existing branch/worktree, and bounded Component Model runtime code context were checked. Depends-on blockers are closed, Plugin CLI / HTTPS / FS related work are closed, and no dirty-state blocker or missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T16:25:23Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `878517dc ticket: accept plugin component model runtime`.
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVG0HR96-plugin-component-model-runtime` on branch `impl/00001KVG0HR96-plugin-component-model-runtime` at `878517dc`.
- Initial `SpawnPod` attempt with only the implementation worktree as readable scope failed because the spawned runtime workspace identity is `/home/hare/Projects/yoi` and that root was not readable under the child scope. No child Pod remained registered.
- Retried with read-only scope for `/home/hare/Projects/yoi` plus write scope limited to the implementation worktree. Coder Pod `yoi-coder-00001KVG0HR96` started successfully. The task explicitly instructs the Coder to edit/build/commit only in the implementation worktree and not to operate in the root/original workspace.

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T17:00:21Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVG0HR96`.

Implementation commit:
- `57bbf14e plugin: implement component model runtime`

Changed areas reported:
- Manifest/runtime metadata: added explicit `kind = "wasm-component"` support with `component` artifact path and `world = "yoi:plugin/tool@1.0.0"`; preserved raw core-Wasm as `kind = "wasm"` + `abi = "yoi-plugin-wasm-1"`; reject mixed raw/component runtime metadata.
- Plugin runtime: added `wasmtime::component` path; Component Tools execute through existing ToolRegistry / Worker Tool path; component imports are fail-closed; grants are checked before instantiation/import use and on host API calls; no WASI fs/network/env exposed.
- Static inspection: reports `wasm-component/yoi:plugin/tool@1.0.0` without reading/executing the artifact.
- Tests: component discovery/registration/execution, host-import grant denial, wrong-world, missing-export, raw-core-Wasm-not-component fail-closed, and existing raw runtime coverage.
- Docs / authoring: updated design/package docs, added WIT files under `resources/plugin/wit/`, and sample author source under `docs/examples/plugin-component-tool/lib.rs`.
- Packaging: added `wasmtime = 45.0.2` with narrow non-default features; updated `Cargo.lock` and `package.nix` `cargoHash`.

Coder validation reported as passing:
- `cargo fmt --check`
- `git diff --check`
- `cargo check`
- `cargo test -p pod feature::plugin::tests -- --nocapture` (`55 passed`)
- `cargo test -p manifest plugin -- --nocapture` (`17 passed`)
- `cargo test -p yoi plugin -- --nocapture` (`11 passed`)
- `nix build .#yoi --no-link`

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean on `impl/00001KVG0HR96-plugin-component-model-runtime`.
- Merge-base with `orchestration` is `878517dc`; Orchestrator branch head is `02006fee`.
- Diff from acceptance is one implementation commit, `57bbf14e`, touching 10 files: `Cargo.lock`, manifest Plugin parser, pod Plugin runtime, `crates/pod/Cargo.toml`, docs, `package.nix`, and WIT/sample files.
- `git diff --check 878517dc..HEAD` produced no diagnostics.
- Diff size is material: about 1568 insertions / 68 deletions; dependency impact note is reviewer focus.

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on Component Model authority boundaries, grant enforcement, no ambient WASI, raw runtime compatibility, inspection not executing code, diagnostics, tests, and packaging/Nix impact.

---
