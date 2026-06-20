<!-- event: create author: "yoi ticket" at: 2026-06-20T13:01:37Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-20T13:02:36Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-20T13:02:36Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T13:28:19Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T13:29:10Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Workspace Dashboard Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- Ticket body は Plugin instance model / registry、new Component instance world、legacy Tool adapters、manifest/static validation、plugin check/list/show reporting、Service/Ingress lifecycle/status、Ingress test path、per-surface grants、docs/templates/PDK updates、validation を詳細に定義している。
- 未解決 relation blocker はない。
- 現在 queued はこの Ticket のみ、inprogress は 0 件、spawned child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は plugin / wasm-component / service / ingress / lifecycle / grants / runtime architecture だが、Ticket は no hidden context injection、ToolRegistry run-stability、legacy Tool compatibility、no ambient WASI network/socket、per-surface grants、host-mediated outputs を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVJHYP4Q` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVJHYP4Q)`: no blockers。
- `TicketOrchestrationPlanQuery(00001KVJHYP4Q)`: no previous plan records; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `7f06e656`。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods are self/peers only; spawned children 0。
  - no matching implementation branch/worktree。

IntentPacket:

Intent:
- Move Plugin runtime semantics from per-Tool artifact execution to host-managed `PluginInstance` / `PluginInstanceRegistry`。
- Treat Tool / Service / Ingress as surfaces of the same Plugin instance, sharing instance state/config/diagnostics while preserving explicit authorization and ordinary visible output paths。
- Preserve existing Tool-only component/raw wasm Plugin packages through compatibility adapters。

Binding decisions / invariants:
- Existing Tool Plugin packages must continue to work through instance registry compatibility path。
- Tool execution remains model/user initiated and returns through ordinary Tool result/history path。
- Service/Ingress must not secretly call model Tools or mutate context/history directly。
- Plugin outputs/events must use Tool results or explicit durable/visible host-mediated paths; no hidden context injection。
- Tool schemas remain run-stable and model-visible only through normal ToolRegistry construction。
- Per-surface grants are independent: Tool, Service, Ingress grants must be validated separately; sharing an instance must not bypass authorization。
- Host APIs remain separately grant-gated。
- No raw ambient WASI network/socket authority。
- Ingress events are bounded typed untrusted inputs。
- If a safe host action path does not exist, expose diagnostics/status rather than inventing unsafe paths。

Requirements / acceptance criteria:
- Add `PluginInstanceRegistry` or equivalent host-managed instance boundary。
- ToolRegistry dispatch goes through Plugin instance handles。
- Add new Component instance world/resource files and Rust PDK support。
- Add legacy Tool component/raw wasm adapters behind the instance registry。
- Extend manifest/static validation for Service/Ingress declarations and runtime compatibility。
- Update `yoi plugin check/list/show` reporting for legacy Tool-only vs instance-capable packages and rejected surfaces。
- Add host-managed start/status/stop lifecycle and bounded diagnostics。
- Add Ingress dispatch API and at least one bounded in-process ingress delivery test path。
- Validate Tool/Service/Ingress grants independently。
- Update docs/templates for instance-oriented authoring。
- Focused tests cover manifest validation, legacy compatibility, instance state persistence across Tool calls, Tool/Ingress shared instance dispatch, grant denial, timeout/trap/failure diagnostics。
- Validation includes `cargo fmt --check`, relevant tests/checks, `git diff --check`, `yoi ticket doctor`, and `nix build .#yoi --no-link`。

Escalate if:
- The instance boundary cannot be implemented without broad Worker/ToolRegistry redesign beyond Ticket scope。
- Preserving legacy Tool runtime while adding instance runtime would substantially distort architecture。
- Safe Service/Ingress host action semantics require a product decision not already specified。
- WIT/PDK interface shape requires a compatibility-breaking public API decision beyond this Ticket。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T13:29:23Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_instance_lifecycle field: state -->

## State changed

Ticket body/thread, relation metadata, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded Plugin instance lifecycle context were checked. There is no unresolved blocking dependency, no inprogress/capacity blocker, and no missing planning decision. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T13:30:38Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `5ec8bae9 ticket: accept plugin instance lifecycle`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVJHYP4Q-plugin-instance-lifecycle` on branch `impl/00001KVJHYP4Q-plugin-instance-lifecycle` at `5ec8bae9`。
- Spawned Coder Pod `yoi-coder-00001KVJHYP4Q` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, unsafe hidden context/action paths, broad ToolRegistry bypasses, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T14:16:45Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVJHYP4Q`.

Implementation commit:
- `147a6005 plugin: add instance lifecycle surface`

Changed areas reported:
- Manifest support for Service/Ingress declarations and permissions。
- New `yoi:plugin/instance@1.0.0` WIT/resource surface。
- Host-managed `PluginInstanceRegistry` / `PluginInstanceHandle` in Pod plugin feature layer。
- Plugin Tool registration/dispatch now routes through instance handles。
- Legacy raw-wasm and `yoi:plugin/tool@1.0.0` component Tool runtimes preserved through registry adapter path。
- Instance lifecycle/status/stop/diagnostics surfaces and bounded Ingress dispatch API added。
- Independent Tool/Service/Ingress grant validation added。
- Static inspection / Plugin CLI diagnostics extended for Service/Ingress eligibility。
- Rust PDK instance-oriented types/trait/macro support added。
- Rust component instance template resources added。
- Plugin component model docs updated。
- Tests added/updated for manifest validation, CLI reporting/templates, legacy compatibility, grant checks, and in-process shared Tool/Ingress dispatch。

Coder validation reported as passing:
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p pod plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `cargo check -p yoi`
- `cargo check -p yoi-plugin-pdk`
- `cargo fmt --check`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Dependency/package impact:
- No new Rust dependencies。
- Added package/resource files under `resources/plugin/templates/rust-component-instance/` and `resources/plugin/wit/yoi-plugin-instance-v1.wit`。
- No `Cargo.lock` or `package.nix` changes required; Nix build succeeded。

Known risks / deferrals:
- Instance Component ABI is intentionally minimal string-JSON host dispatch (`start`, `handle-tool`, `handle-ingress`, `status`, `stop`) and should be reviewed before treating it as stable public low-level ABI。
- Service lifecycle is surfaced as host-managed registration/status/stop diagnostics; no unsafe hidden model/tool/context action path was added。
- Rust PDK instance macro provides author-facing instance helpers, but full cargo-component generated glue ergonomics may need follow-up polish if public PDK shape becomes stricter。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean。
- HEAD is `147a6005`。
- Diff from acceptance `5ec8bae9..HEAD` is one implementation commit touching 12 files, about 1399 insertions / 34 deletions。
- `git diff --check 5ec8bae9..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on instance registry boundary, legacy compatibility, ToolRegistry run-stability, no hidden context/action paths, per-surface grants, Service/Ingress lifecycle semantics, static inspection/CLI reporting, WIT/PDK/template correctness, bounds/resource limits, and validation evidence。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T14:17:31Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVJHYP4Q-r1` against implementation branch `impl/00001KVJHYP4Q-plugin-instance-lifecycle`。
- Review target commit: `147a6005 plugin: add instance lifecycle surface`。
- Review baseline: `5ec8bae9`。
- Reviewer task focuses on instance registry boundary, legacy Tool compatibility, ordinary Tool result/history path, Service/Ingress no hidden action/context paths, ToolRegistry run-stability, per-surface and host API grant independence, no ambient WASI/network authority, bounded ingress/events/diagnostics, static inspection no-exec behavior, WIT/PDK/template coherence, tests, and Nix/package evidence。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---

<!-- event: review author: yoi-reviewer-00001KVJHYP4Q-r1 at: 2026-06-20T14:24:40Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket contract / Orchestrator IntentPacket。
- Implementation diff: `5ec8bae9..147a6005`。
- 主な対象:
  - `crates/pod/src/feature/plugin.rs`
  - `crates/manifest/src/plugin.rs`
  - `crates/yoi/src/plugin_cli.rs`
  - `crates/plugin-pdk/src/lib.rs`
  - `resources/plugin/wit/yoi-plugin-instance-v1.wit`
  - `resources/plugin/templates/rust-component-instance/*`
  - Plugin component docs/templates and focused tests。
- Hidden history/context injection、ToolRegistry bypass、Ingress/Service production call path、ambient WASI/network authority の有無を確認。

Blocking issues:

1. Rust instance PDK/template が buildable ではなく、WIT component instance ABI を実際に export していない。
   - `resources/plugin/templates/rust-component-instance/Cargo.toml` は repository 配下 package だが `[workspace]` がなく、direct authoring check が workspace membership error で失敗する。
   - `resources/plugin/templates/rust-component-instance/src/lib.rs` は `ToolOutput::text(...)` を呼ぶが、`crates/plugin-pdk/src/lib.rs` には `ToolOutput::new`, `ToolOutput::json`, `ToolOutput::summary` しかない。
   - `export_plugin_instance!` は generated WIT bindings / generated `export!` macro for `world instance` を実装していない。raw placeholder `#[unsafe(export_name = "start")]` と private Rust methods を定義するだけで、host が期待する component-model exports (`start`, `handle-tool`, `handle-ingress`, `status`, `stop`) を生成しない。
   - Ticket が要求する WIT/PDK/template coherence と instance-oriented authoring surface を満たしていない。

2. Component instance lifecycle が status/error outputs を parse せず、component `status` export が実質 unused。
   - `PluginInstance::status` は host-side lifecycle/diagnostics のみを返し、component runtime の `status` export を呼ばない。
   - `PluginComponentInstanceRuntime::start` は component `start` export の returned string を捨てている。
   - `PluginComponentInstanceRuntime::stop` も returned string を捨てている。
   - WIT は `status` を export しているが、host-side runtime method がない。
   - Component が `{"error": ...}` を `start` から返しても host が started と扱い得るため、lifecycle/status/diagnostics acceptance criteria を満たしていない。

3. Service/Ingress surfaces が Tool 登録なしでは host-managed Plugin instance を作成・保持しない。
   - `PluginToolFeature::install` の実 install path では Service declarations は metadata only、Ingress declarations は authorization-check only。
   - Instance は Tool registration loop 内でのみ作られる。
   - Service-only / Ingress-only instance-capable plugin は host-managed instance、lifecycle start、status、diagnostics、retained ingress dispatch handle を得られない。
   - `deliver_ingress` は production caller に接続されておらず、focused ingress test は synthetic `TestIngress` instance を使っている。
   - Ticket intent の “Tool / Service / Ingress as surfaces of the same Plugin instance” を満たしていない。

Non-blocking concerns / follow-ups:
- Human `yoi plugin show` output でも Service/Ingress eligibility を明示すると JSON なしでも rejected surfaces が分かりやすい。
- Real instance-component tests が弱い。Rust/PDK generated `yoi:plugin/instance@1.0.0` component の build/execution through start/tool/ingress/status/stop coverage が欲しい。

Reviewer validation:
- `git diff --check 5ec8bae9..HEAD`: passed。
- `cargo fmt --check`: passed。
- `cargo test -p manifest plugin -- --nocapture`: passed。
- `cargo test -p pod plugin -- --nocapture`: passed。
- `cargo test -p yoi plugin -- --nocapture`: passed。
- `cargo check -p yoi`: passed。
- `cargo check -p yoi-plugin-pdk`: passed。
- `yoi ticket doctor`: passed。
- `nix build .#yoi --no-link`: passed。
- Additional focused template validation failed: `CARGO_TARGET_DIR=target/review-template cargo check --manifest-path resources/plugin/templates/rust-component-instance/Cargo.toml` with workspace membership error。

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T14:43:43Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVJHYP4Q`.

New fix commit:
- `870bcc76 plugin: fix instance lifecycle blockers`

Branch commits now:
- `870bcc76 plugin: fix instance lifecycle blockers`
- `147a6005 plugin: add instance lifecycle surface`

Fix summary:
- `crates/plugin-pdk/src/lib.rs`:
  - Reworked instance PDK export macro to use generated `wit-bindgen` `Guest` / `export!` shape instead of raw placeholder exports。
  - Added coherent instance-world helpers/status/error JSON handling。
- `resources/plugin/templates/rust-component-instance/`:
  - Added empty `[workspace]` so template checks as an in-tree standalone authoring package。
  - Fixed template API mismatch by using existing `ToolOutput::json`。
  - Added `wit_bindgen::generate!` for `instance` world and updated export macro usage。
- `crates/pod/src/feature/plugin.rs`:
  - Component instance lifecycle now decodes `start` / `stop` returned JSON and fails closed on returned errors / failed status。
  - Host now calls component `status` export and includes component status in `PluginInstanceStatus`。
  - Service/Ingress-capable installs create and retain host-managed instances even without Tool registration。
  - Added retained registry handle and bounded installed-path Ingress dispatch API。
  - Added tests for component start error handling, component status/stop reporting, service-only instance retention, and installed-path Tool/Ingress shared-instance dispatch。
- No `Cargo.lock`, dependency, or `package.nix` changes。

Coder validation reported as passing:
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p pod plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `cargo check -p yoi`
- `cargo check -p yoi-plugin-pdk`
- `CARGO_TARGET_DIR=target/review-template cargo check --manifest-path resources/plugin/templates/rust-component-instance/Cargo.toml`
- `cargo fmt --check`
- `git diff --check 5ec8bae9..HEAD`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Remaining deferral / risk:
- Instance PDK now uses `wit-bindgen` export glue and template checks, but `yoi:plugin/instance@1.0.0` remains first public shape and should be reviewed before broad external stability promises。

Orchestrator evidence checked before r2 review:
- Implementation worktree is clean。
- HEAD is `870bcc76`。
- Fix diff from `147a6005..HEAD` touches 4 files, about 506 insertions / 119 deletions。
- Overall `git diff --check 5ec8bae9..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r2 verification of the PDK/template export glue, lifecycle status/error parsing, Service/Ingress instance retention, installed-path ingress dispatch, and final acceptance criteria。

---

<!-- event: review author: yoi-reviewer-00001KVJHYP4Q-r1 at: 2026-06-20T14:50:21Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket authority / Orchestrator IntentPacket。
- Fix diff: `147a6005..870bcc76`。
- Overall implementation shape: `5ec8bae9..870bcc76`。
- 主な対象:
  - `crates/plugin-pdk/src/lib.rs`
  - `resources/plugin/templates/rust-component-instance/Cargo.toml`
  - `resources/plugin/templates/rust-component-instance/src/lib.rs`
  - `resources/plugin/wit/yoi-plugin-instance-v1.wit`
  - `crates/pod/src/feature/plugin.rs`
  - `crates/manifest/src/plugin.rs`
  - `crates/yoi/src/plugin_cli.rs`

Previous blocker verification:
- Rust instance PDK/template buildability / WIT export shape: substantially addressed。
  - `export_plugin_instance!` now targets generated `wit-bindgen` `Guest` / `export!` shape。
  - Instance template has empty `[workspace]`, uses `ToolOutput::json`, and includes `wit_bindgen::generate!`。
  - Focused template validation passes。
- Component lifecycle start/status/stop handling: substantially addressed。
  - Host parses lifecycle JSON and fails closed on error/failure status。
  - Component `status` export is called and reflected in instance status。
  - Stop output is decoded/reported。
- Service/Ingress instance retention and installed-path dispatch: substantially addressed。
  - `PluginToolFeature` retains a `PluginInstanceRegistry`。
  - Service/Ingress-capable installs can create/retain instance without Tool registration。
  - Installed-path `dispatch_ingress` exists and tests cover shared Tool/Ingress dispatch。

Blocking issue:

1. Enabled surface selection is not enforced per surface during descriptor/install, so mixed-surface plugins can expose or block the wrong surfaces。
   - `PluginToolFeature` is gated at coarse whole-record level by whether any of Tool/Service/Ingress appears in `record.enabled_surfaces`。
   - Once feature exists, descriptor and install paths iterate over all declared manifest services/tools/ingresses, not only selected/enabled surfaces。
   - Concrete risk:
     - Plugin declaring both Tool and Service with only Service selected can still attempt to authorize/register Tool。
     - Missing Tool grant can fail selected Service install。
     - If Tool grant exists, unselected Tool can become model-visible。
     - Unselected Service/Ingress can be provided/started if their grants exist。
   - This violates the Ticket requirement that Tool / Service / Ingress grants and exposure remain independent and explicitly gated。

Required fix:
- Filter descriptor/install loops by `record.enabled_surfaces`。
- Ensure registered/denial behavior accounts for selected surface set, not all manifest declarations。
- Add focused tests for mixed-surface packages with partial enabled surfaces:
  - Service selected, Tool declared but not selected: Service installs and Tool does not register。
  - Tool selected, Service/Ingress declared but not selected: no Service/Ingress instance/dispatch exposure。
  - Missing grants for unselected surfaces do not fail selected-surface install。
  - Grants for unselected surfaces do not cause exposure。

Non-blocking concern:
- Existing disabled-surface test only clears all enabled surfaces and does not cover mixed-surface partial enablement。

Reviewer validation:
- `cargo fmt --check`: passed。
- `git diff --check 5ec8bae9..HEAD`: passed。
- `CARGO_TARGET_DIR=target/review-template cargo check --manifest-path resources/plugin/templates/rust-component-instance/Cargo.toml`: passed。
- `cargo test -p manifest plugin -- --nocapture`: passed。
- `cargo test -p pod plugin -- --nocapture`: passed。
- `cargo test -p yoi plugin -- --nocapture`: passed。
- `cargo check -p yoi`: passed。
- `cargo check -p yoi-plugin-pdk`: passed。
- `yoi ticket doctor`: passed。
- `nix build .#yoi --no-link`: passed。

Worktree status at end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T14:59:25Z -->

## Implementation report

Coder r2-fix report received from `yoi-coder-00001KVJHYP4Q`.

New fix commit:
- `79ca0f7f plugin: enforce enabled lifecycle surfaces`

Branch commits now:
- `79ca0f7f plugin: enforce enabled lifecycle surfaces`
- `870bcc76 plugin: fix instance lifecycle blockers`
- `147a6005 plugin: add instance lifecycle surface`

Fix summary:
- `crates/pod/src/feature/plugin.rs`:
  - Added per-surface `record.enabled_surfaces` gating for Plugin descriptor construction。
  - Filtered install loops so Tool / Service / Ingress setup only processes selected surfaces。
  - Changed install denial/exposure accounting to consider selected surfaces only。
  - Added runtime dispatch guards so unselected Tool/Ingress surfaces cannot be used even if grants exist。
  - Added focused tests for mixed-surface partial enablement:
    - Service selected, Tool declared but not selected: Service installs, Tool does not register, missing Tool grants do not fail install。
    - Tool selected, Service/Ingress declared but not selected: no Service descriptor/install exposure, Ingress dispatch denied even with grants。
  - Kept previous lifecycle/status/ingress retention tests passing。

Coder validation reported as passing:
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p pod plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `cargo check -p yoi`
- `cargo check -p yoi-plugin-pdk`
- `CARGO_TARGET_DIR=target/review-template cargo check --manifest-path resources/plugin/templates/rust-component-instance/Cargo.toml`
- `cargo fmt --check`
- `git diff --check 5ec8bae9..HEAD`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Orchestrator evidence checked before r3 review:
- Implementation worktree is clean。
- HEAD is `79ca0f7f`。
- Fix diff from `870bcc76..HEAD` touches only `crates/pod/src/feature/plugin.rs`, about 190 insertions / 97 deletions。
- Overall `git diff --check 5ec8bae9..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r3 verification of per-surface enabled selection enforcement and final acceptance criteria。

---
