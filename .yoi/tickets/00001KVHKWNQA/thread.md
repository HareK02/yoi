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

<!-- event: review author: yoi-reviewer-00001KVHKWNQA-r1 at: 2026-06-20T05:21:50Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket body/thread / Orchestrator IntentPacket
- Diff: `5f7f81bd..06287aca`
- 主な対象: `crates/plugin-pdk/*`, embedded template `resources/plugin/templates/rust-component-tool/*`, `resources/plugin/wit/*`, `crates/manifest/src/plugin.rs`, `crates/pod/src/feature/plugin.rs`, workspace/package docs/Nix/Cargo paths。

良い点:
- PDK は guest-side only として提示され、Yoi host/runtime crates への normal dependency は見当たらない。
- docs/templates は host-side Plugin grants が authority boundary であること、crates.io publication / remote template fetch を要求しないことを概ね維持している。
- `ToolError` / `ToolOutput` bounds と runtime decoder test は現在の ToolOutput JSON bridge と整合している。

Blocking issues:
1. PDK/template authoring path が現在の WIT で実際に compile/parse できない。
   - Template/example/docs は次の generated binding path を示している。
     - `resources/plugin/templates/rust-component-tool/src/lib.rs`
     - `docs/examples/plugin-component-tool/lib.rs`
     - `crates/plugin-pdk/src/lib.rs`
   - しかし `resources/plugin/wit/yoi-host-v1.wit` の interface `fs` が `list: func(...)` を使っており、`wit-bindgen` probe で `list` が keyword として扱われ parse error になった。
   - これは “author does not need to hand-write raw pointer/length ABI code” と “template/sample compile or honest validated fixture boundary” の Ticket contract に反する。

2. Embedded template を source tree 内で Cargo package として check できない。
   - `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown` が root workspace 配下の non-member package として失敗した。
   - Template を workspace member にするか、workspace exclude / embedded template 側の empty `[workspace]` 等で standalone fixture として check 可能にする必要がある。
   - 現在の shape tests は `wit_bindgen::generate!` や WIT parse を実行していないため、この問題を捕捉できていない。

Required fix:
- Current WIT を `wit-bindgen` が parse できるよう修正する。`list` は WIT keyword なので `%list` escape など、runtime import name semantics を壊さない形を優先すること。
- Embedded template を in-place または controlled fixture copy で Cargo check できるようにする。
- `wit_bindgen::generate!` against `resources/plugin/wit` を実際に通す test/probe を追加する。string-shape assertion だけでは不可。
- Template/example/PDK docs が full build boundary と deferral を正直に表現していることを確認する。

Reviewer validation:
- Passed:
  - `cargo fmt --check`
  - `git diff --check 5f7f81bd..HEAD`
  - `cargo test -p yoi-plugin-pdk`
  - `cargo test -p manifest embedded_rust_component_tool_template_is_valid_package_shape`
  - `cargo test -p pod pdk_tool_output_shape_is_accepted_by_wasm_decoder`
  - `cargo check`
  - `cargo tree -p yoi-plugin-pdk --edges normal`
  - `nix build .#yoi --no-link`
- Failed reviewer probes:
  - `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown`: workspace membership/standalone failure。
  - copied template probe with adjusted relative paths and empty `[workspace]`: WIT parse failure at `resources/plugin/wit/yoi-host-v1.wit` because `list` is keyword。

Worktree status at review end: source tree clean; ignored artifacts under `target/` only。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T05:40:15Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVHKWNQA`.

New fix commit:
- `0a9e585c plugin: fix rust pdk wit template probes`

Branch commits now:
- `0a9e585c plugin: fix rust pdk wit template probes`
- `06287aca plugin: add rust pdk template`

Fix summary:
- Moved host WIT to dependency package path `resources/plugin/wit/deps/yoi-host/yoi-host-v1.wit` so `resources/plugin/wit` is a valid WIT package directory with `yoi:host` resolved as dependency。
- Escaped WIT keyword function as `%list` while preserving import name semantics。
- Added empty `[workspace]` to embedded template `Cargo.toml` so it can be checked in-place without becoming an undeclared member of Yoi root workspace。
- Updated template/example/docs/PDK snippets to the compile-tested binding pattern with PDK `wit_bindgen` re-export, `generate_all`, and `runtime_path: "yoi_plugin_pdk::wit_bindgen::rt"`。
- Added compile-time probe `crates/plugin-pdk/tests/wit_bindgen_probe.rs` that runs `wit_bindgen::generate!` against `resources/plugin/wit`。
- Added embedded template cargo-check probe for `wasm32-unknown-unknown` in `crates/plugin-pdk/tests/template.rs`。
- Added `%list` WIT assertion and updated host WIT include path。
- Updated `Cargo.lock` and `package.nix` for new dev dependency edge / cargo hash。

Coder validation reported as passing:
- `cargo test -p yoi-plugin-pdk` including PDK tests, WIT constants, actual `wit_bindgen::generate!` probe, and embedded template wasm32 check probe。
- `cargo test -p manifest embedded_rust_component_tool_template_is_valid_package_shape`
- `cargo test -p pod pdk_tool_output_shape_is_accepted_by_wasm_decoder`
- `cargo check`
- `cargo tree -p yoi-plugin-pdk --edges normal`
- `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown`
- `cargo fmt --check`
- `git diff --check 5f7f81bd..HEAD`
- `git diff --check`
- `git diff --check --cached`
- `nix build .#yoi --no-link`

Operational note:
- `nix build .#yoi --no-link` initially hit disk exhaustion after accepting the updated vendor hash; Coder removed local implementation worktree `target/` cache and reran successfully。

Remaining deferrals:
- No `yoi plugin new/check/pack`。
- No crates.io publication or remote template fetching。
- Full packaged component execution remains deferred; the current WIT parses with `wit-bindgen` and embedded Rust template compiles to the wasm target boundary。

Orchestrator evidence checked before r2 review:
- Implementation worktree is clean。
- HEAD is `0a9e585c`。
- Fix diff from `06287aca..HEAD` touches 12 files, about 105 insertions / 13 deletions。
- Overall `git diff --check 5f7f81bd..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r2 verification of the WIT parse/template compile blockers and final acceptance criteria。

---

<!-- event: review author: yoi-reviewer-00001KVHKWNQA-r1 at: 2026-06-20T05:47:13Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket body/thread / Orchestrator IntentPacket
- r2 fix diff: `06287aca..0a9e585c`
- 全体実装 diff: `5f7f81bd..0a9e585c`
- 主な対象: `resources/plugin/wit/deps/yoi-host/yoi-host-v1.wit`, `resources/plugin/wit/yoi-plugin-tool-v1.wit`, embedded template, `crates/plugin-pdk/*`, `crates/manifest/src/plugin.rs`, `crates/pod/src/feature/plugin.rs`, docs, `package.nix`。

Previous blockers verification:
- Host WIT は `resources/plugin/wit/deps/yoi-host/yoi-host-v1.wit` に配置され、`resources/plugin/wit` は current package directory として `wit-bindgen` が parse できる形になった。
- Keyword function は `%list` として escape され、import name semantics を保持している。
- Embedded template の `Cargo.toml` には empty `[workspace]` があり、root workspace 配下の non-member package として拒否されない。
- `crates/plugin-pdk/tests/wit_bindgen_probe.rs` は実際に `wit_bindgen::generate!` を `resources/plugin/wit` に対して実行している。
- Embedded template は `wasm32-unknown-unknown` target で cargo-check できる。

Overall Ticket fit:
- PDK は guest-side に留まり、host/runtime Yoi crates への dependency は見当たらない。
- PDK / template は authority を付与せず、host-side Plugin manifest grants が authority boundary であることを維持している。
- Ambient fs/network/env authority は露出していない。
- crates.io publication / remote template fetch / `yoi plugin new/check/pack` は実装していない。
- Docs は full build/pack boundary と deferral を概ね正直に記述している。

Blocking issues: none。

Non-blocking concerns / follow-ups:
- `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown` を直接実行すると template-local `Cargo.lock` と `target/` ができる。Reviewer は検証後に untracked artifacts を削除した。テスト自体は temporary target dir を使い template lockfile を消すため blocking ではないが、将来 docs/CI では explicit `--target-dir` を推奨するとよい。
- Reviewer r2 の `nix build .#yoi --no-link` は final artifact copy 中に `No space left on device` で environment-fail。cargo hash/source-filter/code diagnostic ではない。Coder 側では同コマンド成功済み。Orchestrator final validation で再確認する。

Reviewer validation:
- `cargo test -p yoi-plugin-pdk`: passed
- `cargo test -p manifest embedded_rust_component_tool_template_is_valid_package_shape`: passed
- `cargo test -p pod pdk_tool_output_shape_is_accepted_by_wasm_decoder`: passed
- `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown`: passed
- `cargo tree -p yoi-plugin-pdk --edges normal`: passed; host/runtime Yoi crate dependencyなし
- `cargo fmt --check`: passed
- `git diff --check 5f7f81bd..HEAD`: passed
- `git diff --check`: passed
- `git diff --check --cached`: passed
- `cargo check`: passed
- `nix build .#yoi --no-link`: attempted, environment failed with `No space left on device` during final artifact copying

Worktree status at review end: source tree clean; ignored artifacts under normal `target/` only。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T05:51:33Z -->

## Implementation report

Merge and final validation report:

Merged implementation branch into Orchestrator branch:
- Merge commit: `edc53a6b merge: plugin rust pdk templates`
- Implementation commits included:
  - `06287aca plugin: add rust pdk template`
  - `0a9e585c plugin: fix rust pdk wit template probes`

Reviewer outcome:
- r1 requested changes for WIT parse failure (`list` keyword) and embedded template Cargo workspace check failure。
- Coder fixed WIT package layout / `%list` escaping, added actual `wit_bindgen::generate!` probe, and made the embedded template standalone-checkable。
- r2 approved with no blocking issues。

Orchestrator validation after merge passed:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p yoi-plugin-pdk`
- `cargo test -p manifest embedded_rust_component_tool_template_is_valid_package_shape`
- `cargo test -p pod pdk_tool_output_shape_is_accepted_by_wasm_decoder`
- `cargo check`
- `cargo tree -p yoi-plugin-pdk --edges normal`
- `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown`
- `nix build .#yoi --no-link`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-o9gvGb.log`

Package impact:
- `nix path-info -S .#yoi`: `112156384`

Cleanup note:
- Direct template cargo-check creates local `resources/plugin/templates/rust-component-tool/Cargo.lock` and `target/`; Orchestrator removed those untracked artifacts after validation. Worktree is clean。

Final state:
- Orchestrator worktree clean at `edc53a6b` after validation。
- Implementation worktree remains available for cleanup after Ticket completion records are committed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T05:51:42Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Implementation was merged into Orchestrator branch at `edc53a6b`, r2 review approved, and final Orchestrator validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, focused PDK/manifest/pod tests, `cargo check`, PDK dependency tree check, embedded template wasm32 check, and `nix build .#yoi --no-link`.

---
