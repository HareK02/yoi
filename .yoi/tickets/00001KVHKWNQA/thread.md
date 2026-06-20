<!-- event: create author: "yoi ticket" at: 2026-06-20T04:16:14Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T04:52:58Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T04:53:59Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body は Rust PDK crate、guest-side only boundary、ergonomic Tool helpers、Component Model binding glue、embedded `rust-component-tool` template、docs/tests/validation/non-goals を実装可能な粒度で定義している。
- Blocking dependency `00001KVG0HR96` Component Model runtime は closed。Component runtime は explicit `wasm-component` metadata、ToolRegistry path、grant enforcement、no ambient WASI、resource limits、WIT files を含めて完了済み。
- Incoming dependency from `00001KVHKWNQS` は、この Ticket が将来の authoring CLI Ticket を unblock する関係であり、本 Ticket の blocker ではない。
- Related Plugin CLI / HTTPS / FS Tickets は closed または non-blocking context。
- 現在 queued はこの Ticket のみ、inprogress は 0 件、child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は plugin / pdk / component-model / authoring / templates / sdk / no-crates-io だが、Ticket は no crates.io publication、no remote template fetch、guest-side only、no ambient authority、host-side grants remain authority などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHKWNQA` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHKWNQA)`: outgoing `depends_on` target `00001KVG0HR96` is closed。incoming `00001KVHKWNQS depends_on this` is not a blocker。
- `TicketOrchestrationPlanQuery(00001KVHKWNQA)`: no previous plan records; accepted plan was recorded now。
- Workspace state:
  - Orchestrator worktree clean at `9ca2f85b`。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
  - no matching implementation branch/worktree。
- Code/resource context:
  - `resources/plugin/wit/yoi-plugin-tool-v1.wit` and `resources/plugin/wit/yoi-host-v1.wit` exist from the Component runtime work。
  - `docs/development/plugin-development.md` is the current Plugin development guide target。
  - Cargo workspace has no existing `crates/plugin-pdk` crate。

IntentPacket:

Intent:
- Add a first-party Rust PDK for Component Model Tool Plugins and an embedded starter template so authors can implement Yoi Plugin Tools without raw pointer/length ABI plumbing or remote template fetches。
- Position Component Model + PDK as preferred authoring path while keeping raw core-Wasm ABI as compatibility/transitional runtime support。

Binding decisions / invariants:
- PDK is guest-side only。It must not depend on host runtime crates such as `pod`, `llm-worker`, `tui`, or `client`。
- PDK must not grant or imply authority。Host-side Plugin manifest grants remain the authority boundary for Tool execution / HTTPS / FS host APIs。
- No ambient fs/network/env authority is introduced。
- Do not publish to crates.io and do not implement remote template fetch。
- Do not implement `yoi plugin new/check/pack` in this Ticket; embedded resources should be suitable for that future Ticket。
- Template dependency policy must support checkout/dev local path dependency and document a future out-of-tree git `rev` pattern。
- PDK should target the current Component Model Tool world (`yoi:plugin/tool@1.0.0`) and runtime ToolOutput JSON bridge。
- Prefer minimal, testable helper APIs over broad macro magic。

Requirements / acceptance criteria:
- Add workspace crate `yoi-plugin-pdk` under `crates/plugin-pdk` or a justified equivalent。
- Provide typed JSON input parsing, output serialization, structured/bounded `ToolError`, `ToolContext` with at least tool name, and helper equivalent to `run_json_tool` producing ToolOutput JSON accepted by the current runtime。
- Provide WIT binding glue/re-export/wrapper enough that authors do not hand-write raw pointer/length ABI code。
- Add embedded template under `resources/plugin/templates/rust-component-tool/` with `Cargo.toml`, `src/lib.rs`, `plugin.toml`, and README/next steps。
- Include example or fixture Plugin using the PDK。
- Update Plugin development docs and explain publication/template-fetch deferrals。
- Tests cover PDK happy/error paths, template/sample fixture validation, runtime acceptance if feasible, and no host-runtime crate dependency。
- Validation includes focused PDK/plugin tests, `cargo fmt --check`, relevant `cargo check`/`cargo test`, `git diff --check`, and `nix build .#yoi` because workspace/package/resources may change。

Escalate if:
- Current WIT/runtime shape cannot support a guest-side helper without changing the Component runtime contract。
- A full sample component build requires external tooling that cannot be kept deterministic in this repo。
- Keeping the PDK guest-only would require undesirable dependencies on host crates。
- Template/resource packaging needs a broader CLI/resource architecture decision。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating the implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T04:54:04Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_rust_pdk_templates field: state -->

## State changed

Ticket body/thread, relation metadata, Component Model runtime dependency state, related completed Tickets, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded resource/code context were checked. Dependency `00001KVG0HR96` is closed, no inprogress/capacity blocker exists, and no missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T04:54:51Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `5f7f81bd ticket: accept plugin rust pdk templates`.
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHKWNQA-plugin-rust-pdk-templates` on branch `impl/00001KVHKWNQA-plugin-rust-pdk-templates` at `5f7f81bd`.
- Spawned Coder Pod `yoi-coder-00001KVHKWNQA` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity.
- Coder task explicitly forbids root/original workspace implementation, merge, Ticket close, and cleanup.

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T05:16:16Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHKWNQA`.

Implementation commit:
- `06287aca plugin: add rust pdk template`

Changed areas reported:
- Added workspace crate `crates/plugin-pdk` / `yoi-plugin-pdk`:
  - guest-side PDK helpers;
  - typed JSON input parsing via `parse_json_input` / `run_json_tool`;
  - typed JSON output via `ToolOutput::json`;
  - bounded structured `ToolError` and error-code helpers;
  - `ToolContext` with selected tool name;
  - `wit_bindgen` re-export and `export_component_tool!` macro to avoid raw pointer/length ABI plumbing;
  - tests for happy path, error path, oversized output, template validation, and host-runtime dependency exclusion。
- Added embedded starter template under `resources/plugin/templates/rust-component-tool/` with `Cargo.toml`, `src/lib.rs`, `plugin.toml`, and `README.md`。
- Added embedded template constants in `crates/manifest/src/plugin.rs` for future authoring CLI use without remote fetching。
- Updated Component Model example to use the PDK。
- Added runtime decoder test confirming PDK-produced ToolOutput JSON shape is accepted。
- Updated Plugin development/design/package docs。
- Updated workspace/package metadata: root `Cargo.toml`, `Cargo.lock`, `package.nix` cargo hash。

Coder validation reported as passing:
- `cargo test -p yoi-plugin-pdk`
- `cargo test -p manifest embedded_rust_component_tool_template_is_valid_package_shape`
- `cargo test -p pod pdk_tool_output_shape_is_accepted_by_wasm_decoder`
- `cargo check`
- `cargo fmt --check`
- `git diff --check`
- `git diff --check --cached`
- `cargo tree -p yoi-plugin-pdk --edges normal`
- `nix build .#yoi --no-link`

Known deferrals reported:
- No `yoi plugin new/check/pack`, remote template fetch, or crates.io publication。
- Full deterministic sample component build/pack execution remains deferred to future authoring CLI/tooling; current sample/template are fixture-validated and use current WIT/PDK shape。
- Raw core-Wasm ABI remains compatibility/transitional runtime support。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean on `impl/00001KVHKWNQA-plugin-rust-pdk-templates`。
- HEAD is `06287aca`。
- Diff from acceptance `5f7f81bd..HEAD` is one implementation commit touching 17 files, about 857 insertions / 35 deletions。
- `git diff --check 5f7f81bd..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on guest-only PDK boundary, no host-runtime deps, authority/grant wording, template/resource packaging, sample/test boundary honesty, docs, and Nix/workspace changes。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T05:16:53Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHKWNQA-r1` against implementation branch `impl/00001KVHKWNQA-plugin-rust-pdk-templates`。
- Review target commit: `06287aca plugin: add rust pdk template`。
- Review baseline: `5f7f81bd`。
- Reviewer task focuses on guest-only PDK boundary, absence of host-runtime deps, authority/grant wording, no ambient fs/network/env, template/resource packaging, current WIT/runtime compatibility, sample/test honesty, docs, and Nix/workspace changes。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---
