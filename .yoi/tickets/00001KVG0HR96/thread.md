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

<!-- event: plan author: yoi-orchestrator at: 2026-06-19T17:00:57Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVG0HR96-r1` against implementation branch `impl/00001KVG0HR96-plugin-component-model-runtime`.
- Review target commit: `57bbf14e plugin: implement component model runtime`.
- Review baseline: `878517dc`.
- Reviewer task focuses on Component/WIT authority boundaries, Plugin grant enforcement, no ambient WASI, raw runtime compatibility, inspection read-only behavior, fail-closed diagnostics, HTTPS/FS invariant preservation, dependency/package impact, and validation evidence.
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator.

---

<!-- event: review author: yoi-reviewer-00001KVG0HR96-r1 at: 2026-06-19T17:08:18Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket contract: `.yoi/tickets/00001KVG0HR96/item.md`, `.yoi/tickets/00001KVG0HR96/thread.md`
- Diff: `878517dc..57bbf14e`
- 主な対象: `crates/manifest/src/plugin.rs`, `crates/pod/src/feature/plugin.rs`, `crates/pod/Cargo.toml`, `Cargo.lock`, `package.nix`, `resources/plugin/wit/*`, docs/sample, existing `crates/yoi/src/plugin_cli.rs` static inspection path。

良い点:
- 明示的な `wasm-component` metadata、raw `wasm` ABI との分離、artifact を実行しない static inspection、ToolRegistry 経由の registration/execution、Tool execution/host API 前の grant check、WASI imports を link しない方針は概ね Ticket intent に沿っている。

Blocking issue:
1. Component runtime が既存 WASM sandbox/resource boundary を保持していない。
   - Raw core-Wasm path は `StoreLimitsBuilder` / `memory_size(PLUGIN_WASM_MEMORY_BYTES)` / table/instance/memory limits / `store.limiter(...)` を使っている。
   - 新しい Component Model path (`crates/pod/src/feature/plugin.rs:1965-2006`) は `consume_fuel(true)` と `max_wasm_stack(...)` は設定しているが、component execution store に同等の `Store::limiter(...)` / resource limit がない。
   - また component result は `String` として host 側に lift された後で `decode_plugin_wasm_output(output.as_bytes())` による output cap を適用しており、untrusted component が output cap 前に host allocation を強制できる。
   - これは Ticket/design の「Component Model support must preserve existing Plugin authority/sandbox boundaries」に反する。component memory/table/instance/output bounds のテストも不足している。

Required fix:
- Component execution store に raw runtime と同等の Wasmtime resource limiter / memory/table/instance limits を入れる。
- Component output/string lifting が unbounded host allocation にならないよう、少なくとも component memory/resource limit により bounded で fail-closed になることを実装・コメント・テストで示す。
- Component memory/resource/output bound の negative tests を追加する。

Non-blocking concerns / follow-ups:
- WIT は現在 `call(tool-name: string, input-json: string) -> string`。Ticket の “Tool request / response / structured error types” は完全には表現していない。今回 v1 の narrow ABI として意図的に deferred なら decision/follow-up として記録すること。
- Rust sample は `wit_bindgen` authoring sketch であり、build/package された sample plugin を test しているわけではない。follow-up として packaged sample があるとよい。
- `nix build` / `cargoHash` は検証されているが、binary size/build-time impact measurement がまだ明示記録されていない。最終 closure 前に capture すること。

Reviewer validation:
- `git diff --check 878517dc..57bbf14e`: passed
- `cargo fmt --check`: passed
- `cargo test -p manifest plugin -- --nocapture`: 17 passed
- `cargo test -p pod feature::plugin::tests -- --nocapture`: 55 passed
- `cargo test -p yoi plugin -- --nocapture`: 11 passed
- `cargo check`: passed
- `nix build .#yoi --no-link`: passed
- `nix path-info .#yoi`: `/nix/store/m5gsv05lfs7s9bh8dfy4ady6sda925fv-yoi-0.1.0`

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T17:17:08Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVG0HR96`.

New fix commit:
- `a705bb3b plugin: bound component model runtime resources`

Branch commits now:
- `a705bb3b plugin: bound component model runtime resources`
- `57bbf14e plugin: implement component model runtime`

Fix summary:
- Added `wasmtime::StoreLimits` to Component Model host state.
- Applied raw-runtime-equivalent limits to component execution store:
  - `memory_size(PLUGIN_WASM_MEMORY_BYTES)`
  - `table_elements(PLUGIN_WASM_TABLE_ELEMENTS)`
  - `instances(1)`
  - `tables(1)`
  - `memories(1)`
  - `trap_on_grow_failure(true)`
- Added inline comment near WIT string lift explaining that host allocation before `decode_plugin_wasm_output` is bounded by component memory/table/instance limits.
- Added negative tests:
  - component oversized memory fails closed before string lift
  - component oversized table fails closed
  - component output cap still fails closed after bounded lift
- Added docs note that JSON-string WIT v1 request/response is deliberate migration bridge and structured WIT records are deferred.

Coder validation reported as passing:
- `cargo fmt --check`
- `git diff --check`
- `cargo check`
- `cargo test -p pod feature::plugin::tests -- --nocapture` (`58 passed`)
- `cargo test -p manifest plugin -- --nocapture` (`17 passed`)
- `cargo test -p yoi plugin -- --nocapture` (`11 passed`)
- `nix build .#yoi --no-link`

Package / binary impact measurement reported:
- `nix path-info -S .#yoi`: `/nix/store/g9nhx3g2xsn7r3rw6n61scdkha5s7m8s-yoi-0.1.0 112156120`
- `stat -c '%s %n' "$out/bin/yoi"`: `54605944 .../bin/yoi`
- `du -sh "$out"`: `53M`

Orchestrator evidence checked before re-review:
- Implementation worktree is clean.
- HEAD is `a705bb3b`.
- Fix diff from `57bbf14e..HEAD` touches `crates/pod/src/feature/plugin.rs` and `docs/design/plugin-component-model.md`, 108 insertions.
- Overall `git diff --check 878517dc..HEAD` produced no diagnostics.

Next action:
- Route back to Reviewer for r2 verification of the resource-boundary blocker and final acceptance criteria.

---

<!-- event: review author: yoi-reviewer-00001KVG0HR96-r1 at: 2026-06-19T17:19:55Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket contract: `.yoi/tickets/00001KVG0HR96/item.md`, `.yoi/tickets/00001KVG0HR96/thread.md`
- r1 blocker fix diff: `57bbf14e..a705bb3b`
- 全体実装: `878517dc..a705bb3b`
- 主な対象: Plugin runtime / manifest / WIT / docs / tests / Cargo/Nix/package impact paths。

Previous blocker verification:
- Component runtime now has raw-runtime-equivalent Wasmtime store limits:
  - `crates/pod/src/feature/plugin.rs:1514-1523`
  - `memory_size(PLUGIN_WASM_MEMORY_BYTES)`
  - `table_elements(PLUGIN_WASM_TABLE_ELEMENTS)`
  - `instances(1)`
  - `tables(1)`
  - `memories(1)`
  - `trap_on_grow_failure(true)`
- Component host state carries those limits:
  - `crates/pod/src/feature/plugin.rs:1926-1930`
- Component store installs the limiter before instantiation/execution:
  - `crates/pod/src/feature/plugin.rs:1991-2001`
- Code documents why WIT string lifting remains bounded before ToolOutput JSON cap enforcement:
  - `crates/pod/src/feature/plugin.rs:2017-2026`
- Focused negative tests were added and passed:
  - component memory limit fails closed before string lift
  - component table limit fails closed
  - component output cap still fails closed after bounded lift

Overall Ticket satisfaction:
- Runtime kind is explicit and manifest-driven: raw `kind = "wasm"` + `abi = "yoi-plugin-wasm-1"`; component `kind = "wasm-component"` + `component` + expected `world`。
- Component execution stays on existing Plugin Tool / ToolRegistry path。
- Plugin grants remain authority boundary before Tool execution and host API use。
- Component imports are validated; unsupported imports fail closed。
- No WASI fs/network/env is linked。
- Static inspection reports runtime metadata without executing component artifacts。
- Existing raw runtime tests continue passing。
- Docs record JSON-string WIT v1 as deliberate migration bridge and defer structured WIT records:
  - `docs/design/plugin-component-model.md:169-177`

Blocking issues: none。

Non-blocking follow-ups:
- Structured WIT request/response/error records are intentionally deferred。
- Sample remains an authoring sketch rather than fully built packaged sample, but runtime behavior is covered by focused component fixtures/tests。

Reviewer validation:
- `git diff --check 878517dc..a705bb3b`: passed
- `cargo fmt --check`: passed
- `cargo test -p pod feature::plugin::tests -- --nocapture`: 58 passed
- `cargo test -p manifest plugin -- --nocapture`: 17 passed
- `cargo test -p yoi plugin -- --nocapture`: 11 passed
- `cargo check`: passed
- `nix build .#yoi --no-link`: passed
- `nix path-info -S .#yoi`: `/nix/store/g9nhx3g2xsn7r3rw6n61scdkha5s7m8s-yoi-0.1.0 112156120`
- `stat -c '%s %n' "$out/bin/yoi"`: `54605944 .../bin/yoi`
- `du -sh "$out"`: `53M`

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-19T17:23:09Z -->

## Implementation report

Merge and final validation report:

Merged implementation branch into Orchestrator branch:
- Merge commit: `63d7ad78 merge: plugin component model runtime`
- Implementation commits included:
  - `57bbf14e plugin: implement component model runtime`
  - `a705bb3b plugin: bound component model runtime resources`

Reviewer outcome:
- r1 requested changes for missing Component Model resource limiter.
- Coder added raw-runtime-equivalent Wasmtime `StoreLimits` for Component execution and negative tests.
- r2 approved with no blocking issues.

Orchestrator validation after merge passed:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo check`
- `cargo test -p pod feature::plugin::tests -- --nocapture`
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `nix build .#yoi --no-link`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-rZDseu.log`

Package impact recorded by Coder/Reviewer:
- `nix path-info -S .#yoi`: `112156120`
- `bin/yoi` size: `54605944`
- output dir: `53M`

Final state:
- Orchestrator worktree clean at `63d7ad78` after validation.
- Implementation worktree remains available for cleanup after Ticket completion records are committed.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-19T17:23:13Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Implementation was merged into Orchestrator branch at `63d7ad78`, r2 review approved, and final Orchestrator validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo check`, focused Plugin tests for `pod`/`manifest`/`yoi`, and `nix build .#yoi --no-link`.

---

<!-- event: state_changed author: hare at: 2026-06-19T17:23:31Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-19T17:23:31Z status: closed -->

## 完了

## Resolution

`00001KVG0HR96` を完了しました。

実装内容:
- Plugin manifest/runtime metadata に明示的な Component Model runtime (`kind = "wasm-component"`) を追加しました。
- 既存 raw core-Wasm runtime (`kind = "wasm"`, `abi = "yoi-plugin-wasm-1"`) は明示的に維持しました。
- `wasmtime::component` による Component Tool execution path を追加しました。
- Component Tool は既存 ToolRegistry / Worker Tool path を通って実行され、hidden context injection はありません。
- WIT host imports は権限そのものではなく、Plugin grants が Tool execution / host API use の authority boundary のままです。
- Component runtime に raw runtime 相当の Wasmtime resource limits を追加し、memory/table/instance/output bound の negative tests を追加しました。
- WASI fs/network/env は expose していません。
- `yoi plugin list/show` static inspection は Component runtime metadata を報告し、component artifact を実行しません。
- WIT files、Component sample authoring sketch、docs/design updates、package/Nix updates を追加しました。
- JSON-string WIT v1 request/response shape は migration bridge として docs に記録し、structured records は follow-up に deferred としました。

主な commit:
- `57bbf14e plugin: implement component model runtime`
- `a705bb3b plugin: bound component model runtime resources`
- `63d7ad78 merge: plugin component model runtime`

Review:
- r1 は Component runtime resource limit 不足で `request_changes`。
- Coder が resource limiter / negative tests / docs note を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo check`
- `cargo test -p pod feature::plugin::tests -- --nocapture`
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `112156120`
- `bin/yoi`: `54605944`
- output dir: `53M`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-rZDseu.log`

---
