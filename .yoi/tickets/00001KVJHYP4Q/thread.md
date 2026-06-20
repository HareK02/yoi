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

<!-- event: review author: yoi-reviewer-00001KVJHYP4Q-r1 at: 2026-06-20T15:04:08Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket authority / Orchestrator IntentPacket。
- r3 fix diff: `870bcc76..79ca0f7f`。
- Relevant overall implementation shape: `5ec8bae9..79ca0f7f`。
- 主な対象:
  - `crates/pod/src/feature/plugin.rs`
  - `crates/yoi/src/plugin_cli.rs`
  - `crates/manifest/src/plugin.rs`
  - `crates/plugin-pdk/src/lib.rs`
  - `resources/plugin/templates/rust-component-instance/*`
  - `resources/plugin/wit/yoi-plugin-instance-v1.wit`

Previous r2 blocker verification:
- Runtime/registration side is mostly fixed。
- Descriptor construction is gated by `record.enabled_surfaces`。
- Install loops now gate Service / Ingress / Tool setup independently。
- Install denial accounting counts selected exposed surfaces rather than only Tools。
- Runtime dispatch guards exist for unselected Tool/Ingress use。
- Tests cover requested selected/unselected mixed-surface cases。

Blocking issue:

1. Static inspection / `yoi plugin list/show` still evaluates unselected surfaces and can mark valid partial-surface enablement as rejected。
   - Runtime install now filters enabled surfaces, but static inspection still treats all declared manifest Tool / Service / Ingress surfaces as active eligibility requirements。
   - `PluginStaticInspection::statically_eligible()` requires all inspected tools, services, and ingresses to be eligible。
   - `inspect_resolved_plugin_static()` still iterates over all declared manifest tools/services/ingresses rather than selected/enabled surfaces。
   - `yoi plugin list/show` uses this unfiltered static inspection for resolved items。
   - Status calculation can classify the resolved item as `rejected` when no usable Tool exists and `static_eligible` is false。

Concrete failure mode:
- Mixed package declares Tool + Service。
- Enablement selects only Service。
- Service grants are present; Tool grants are absent because Tool is not selected。
- Runtime install correctly ignores unselected Tool。
- Static inspection still checks unselected Tool and records missing Tool grant diagnostics, so `static_eligible = false`。
- For service-only selection with no usable Tool, `yoi plugin list/show` can report the resolved plugin as `rejected` even though selected Service surface is valid/installable。

Why this blocks approval:
- Ticket requires Tool / Service / Ingress grants to be independent。
- Unselected surfaces must not block selected surfaces。
- Ticket also requires `yoi plugin check/list/show` to report legacy vs instance-capable/rejected surfaces accurately。
- Runtime path is fixed, but inspection/status can still be blocked by unselected surfaces, creating authority/reporting mismatch。

Required fix:
- Make resolved static inspection eligibility account for `record.enabled_surfaces`。
- Alternatively, separate declared-surface inspection from enabled-surface inspection and compute resolved `list/show` status from enabled surfaces only。
- Keep `plugin check` free to inspect full package declaration if appropriate, but `plugin list/show` for a resolved enablement must not mark selected Service/Ingress invalid because unrelated unselected Tool grants are absent。
- Add focused `yoi plugin` tests for mixed-surface partial enablement status/reporting。

Non-blocking concern:
- Runtime install tests now cover partial enablement, but focused `yoi plugin list/show` tests are still missing and should be added with the fix。

Reviewer validation:
- `cargo fmt --check`: passed。
- `git diff --check 5ec8bae9..HEAD`: passed。
- Template cargo-check: passed。
- `cargo test -p manifest plugin -- --nocapture`: passed。
- `cargo test -p pod plugin -- --nocapture`: passed。
- `cargo test -p yoi plugin -- --nocapture`: passed。
- `cargo check -p yoi`: passed。
- `cargo check -p yoi-plugin-pdk`: passed。
- `yoi ticket doctor`: passed。
- `nix build .#yoi --no-link`: passed。

Worktree status at end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T15:13:56Z -->

## Implementation report

Coder r3-fix report received from `yoi-coder-00001KVJHYP4Q`.

New fix commit:
- `627c8f36 plugin: filter static enabled surfaces`

Branch commits now:
- `627c8f36 plugin: filter static enabled surfaces`
- `79ca0f7f plugin: enforce enabled lifecycle surfaces`
- `870bcc76 plugin: fix instance lifecycle blockers`
- `147a6005 plugin: add instance lifecycle surface`

Fix summary:
- `crates/pod/src/feature/plugin.rs`:
  - `inspect_resolved_plugin_static()` now filters Tool / Service / Ingress inspection by `record.enabled_surfaces`。
  - `PluginStaticInspection::statically_eligible()` therefore reflects enabled/selected surfaces only for resolved Plugin records。
  - `plugin check` remains full-declaration oriented because check-time inspection uses all declared manifest surfaces as enabled。
- `crates/yoi/src/plugin_cli.rs`:
  - Resolved `plugin list/show` diagnostics now use filtered static inspection。
  - Added focused CLI inspection test for mixed Tool+Service package where only Service is enabled and Tool grants are absent:
    - status remains `active`。
    - static eligibility remains true。
    - unselected Tool is not listed/reported。
    - unselected Tool missing grants do not produce diagnostics/rejection。

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

Orchestrator evidence checked before r4 review:
- Implementation worktree is clean。
- HEAD is `627c8f36`。
- Fix diff from `79ca0f7f..HEAD` touches `crates/pod/src/feature/plugin.rs` and `crates/yoi/src/plugin_cli.rs`, about 209 insertions / 76 deletions。
- Overall `git diff --check 5ec8bae9..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r4 verification of static inspection / `plugin list/show` partial-surface reporting and final acceptance criteria。

---

<!-- event: review author: yoi-reviewer-00001KVJHYP4Q-r1 at: 2026-06-20T15:18:34Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket contract:
  - `.yoi/tickets/00001KVJHYP4Q/item.md`
  - `.yoi/tickets/00001KVJHYP4Q/thread.md`
- r4 fix diff: `79ca0f7f..627c8f36`
- overall implementation shape: `5ec8bae9..627c8f36`
- Focused files:
  - `crates/pod/src/feature/plugin.rs`
  - `crates/yoi/src/plugin_cli.rs`
  - `crates/manifest/src/plugin.rs`
  - `crates/plugin-pdk/src/lib.rs`
  - `resources/plugin/templates/rust-component-instance/*`
  - `resources/plugin/wit/yoi-plugin-instance-v1.wit`

r3 blocker verification:
- Previous static-inspection/list-show blocker is resolved。
- `inspect_resolved_plugin_static()` now filters Tool / Service / Ingress eligibility by `record.enabled_surfaces`。
- `PluginStaticInspection::statically_eligible()` now operates over filtered/selected surface vectors for resolved records。
- `plugin check` remains full-declaration oriented because check-time inspection constructs the record with all declared manifest surfaces enabled。
- Resolved `plugin list/show` diagnostics now use filtered static inspection and include selected Service/Ingress diagnostics。
- Focused CLI test `service_only_enablement_ignores_unselected_tool_static_grants` confirms a mixed Tool+Service package with only Service enabled remains `active`, is statically eligible, does not report the unselected Tool, and does not emit missing Tool grant diagnostics。

Earlier blocker re-check:
- Instance PDK/template shape, lifecycle returned JSON/status handling, Service/Ingress instance retention, installed-path ingress dispatch, and per-surface runtime guards remain present at a high level。

Blocking issues:
- None found。

Non-blocking follow-ups:
- None requiring this Ticket to remain blocked。

Reviewer validation passed:
- `cargo fmt --check`
- `git diff --check 5ec8bae9..HEAD`
- `CARGO_TARGET_DIR=target/review-template cargo check --manifest-path resources/plugin/templates/rust-component-instance/Cargo.toml`
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p pod plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `cargo check -p yoi`
- `cargo check -p yoi-plugin-pdk`
- `/home/hare/Projects/yoi/target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link`

Template check generated `resources/plugin/templates/rust-component-instance/Cargo.lock`; reviewer removed it after validation。

Worktree status:
- Clean at commit `627c8f36` on `impl/00001KVJHYP4Q-plugin-instance-lifecycle`。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T15:22:49Z -->

## Implementation report

Reviewer r4 approvalを受けて、実装ブランチを Orchestrator worktree の `orchestration` branch に統合した。

統合:
- Merge commit: `43c9216e merge: plugin instance lifecycle surface`
- Merged branch: `impl/00001KVJHYP4Q-plugin-instance-lifecycle`
- Implementation commits:
  - `147a6005 plugin: add instance lifecycle surface`
  - `870bcc76 plugin: fix instance lifecycle blockers`
  - `79ca0f7f plugin: enforce enabled lifecycle surfaces`
  - `627c8f36 plugin: filter static enabled surfaces`

Orchestrator worktree validation:
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -p manifest plugin -- --nocapture`: passed
- `cargo test -p pod plugin -- --nocapture`: passed
- `cargo test -p yoi plugin -- --nocapture`: passed
- `cargo check -p yoi`: passed
- `cargo check -p yoi-plugin-pdk`: passed
- `CARGO_TARGET_DIR=target/review-template cargo check --manifest-path resources/plugin/templates/rust-component-instance/Cargo.toml`: passed
- `/home/hare/Projects/yoi/target/debug/yoi ticket doctor`: passed
- `nix build .#yoi --no-link`: passed

Template validation generated `resources/plugin/templates/rust-component-instance/Cargo.lock`; it was removed after validation. Final Orchestrator worktree status is clean。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T15:22:53Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Reviewer approval、Orchestrator worktree への統合、必要な検証が完了したため `done` に遷移する。実装は Plugin instance lifecycle surface、Service/Ingress declarations and grants、PDK/template、static/runtime inspection、legacy compatibility、および focused tests を含む。

---

<!-- event: state_changed author: hare at: 2026-06-20T15:23:11Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-20T15:23:11Z status: closed -->

## 完了

Plugin Service/Ingress component lifecycle surface を実装し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- Pod plugin feature に host-managed `PluginInstanceRegistry` / instance handle 境界を追加し、Tool dispatch を instance 経由に変更。
- New instance-capable component world `yoi:plugin/instance@1.0.0` と WIT resource を追加。
- `yoi-plugin-pdk` と Rust component template に instance-oriented authoring support を追加。
- Existing component Tool world / raw wasm Tool runtime を instance registry compatibility path に維持。
- Manifest/static validation に Service / Ingress declarations と per-surface grant validation を追加。
- Service lifecycle/status/diagnostics と bounded in-process ingress dispatch path を実装。
- Tool / Service / Ingress enabled-surface filtering を runtime install, dispatch guard, and resolved static inspection / `yoi plugin list/show` に適用。
- `plugin check` は package declaration inspection、resolved `plugin list/show` は selected/enabled surfaces に基づく reporting に分離。
- Focused tests added for manifest validation, legacy Tool compatibility, instance state persistence, ingress dispatch, Service/Ingress grant denial, failure diagnostics, and partial enabled-surface static reporting。

統合・検証:
- Merge commit: `43c9216e merge: plugin instance lifecycle surface`
- Implementation commits: `147a6005`, `870bcc76`, `79ca0f7f`, `627c8f36`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p manifest plugin -- --nocapture`, `cargo test -p pod plugin -- --nocapture`, `cargo test -p yoi plugin -- --nocapture`, `cargo check -p yoi`, `cargo check -p yoi-plugin-pdk`, template cargo-check, `yoi ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Discord Bridge 本体、public registry/install/update/signature tooling、arbitrary Plugin UI channel、hidden context injection、Service/Ingress による model-visible Tool schema mutation は実装していない。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T15:23:35Z -->

## Implementation report

Post-close cleanup completed。

- Stopped child Pods and reclaimed scope:
  - `yoi-coder-00001KVJHYP4Q`
  - `yoi-reviewer-00001KVJHYP4Q-r1`
- Removed implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVJHYP4Q-plugin-instance-lifecycle`
- Deleted implementation branch:
  - `impl/00001KVJHYP4Q-plugin-instance-lifecycle`
- Orchestrator worktree remains clean on `orchestration` at `bc484338`。

Root/original workspace was not used for merge/validation/cleanup operations beyond observing the worktree list output from the Orchestrator worktree command。

---
