<!-- event: create author: "yoi ticket" at: 2026-06-20T04:16:14Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-20T05:23:14Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T05:23:44Z -->

## Decision

Routing decision: blocked_pending_dependency

Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可されたが、実装開始はまだ行わない。

Reason:
- `00001KVHKWNQS` は `00001KVHKWNQA` に `depends_on` している。
- `00001KVHKWNQA` は現在 `inprogress` で、Rust PDK / embedded template 実装が r1 review の `request_changes` 修正ループ中。
- この Ticket の `new/check/pack` は PDK crate / embedded template を使うため、PDK/template が closed になるまで authoring CLI implementation の worktree/Pod side effects は開始しない。
- Orchestrator workspace は clean、queued はこの Ticket のみ、inprogress は `00001KVHKWNQA` のみ、matching branch/worktree は存在しない。

Action:
- `00001KVHKWNQS` は `queued` のまま保持する。
- `00001KVHKWNQA` が closed になった後、改めて body/thread/relations/workspace state を確認して routing する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-20T05:54:38Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により、この Ticket は Orchestrator routing 対象として明示許可された。
- 前回は `00001KVHKWNQA` Rust PDK/templates が inprogress だったため blocked/queued hold としたが、現在 `00001KVHKWNQA` は closed。
- Ticket body は `new` / `check` / `pack` の CLI surface、non-execution safety、archive safety、deterministic digest/package、JSON reports、diagnostics、tests、validation、non-goals を実装可能な粒度で定義している。
- Related Plugin CLI inspection (`00001KVFD3YSV`) と Component runtime (`00001KVG0HR96`) は closed。
- 現在 queued はこの Ticket のみ、inprogress は 0 件、child implementation Pods はなし、matching branch/worktree はなし、Orchestrator worktree は clean。
- Risk domain は plugin / CLI / authoring / templates / package-validation / packaging / read-only-check だが、Ticket は check/pack が Plugin code を実行しない、new は embedded templates only、enablement config を mutate しない、safe overwrite refusal、archive traversal/root-escape rejection などの invariants を明示している。bounded context check 後も implementation 前に必要な追加 human decision は見つからなかった。

Evidence checked:
- Ticket `00001KVHKWNQS` body / thread / relations / artifacts。
- `TicketRelationQuery(00001KVHKWNQS)`: outgoing `depends_on 00001KVHKWNQA` is now closed。Related records are closed context。
- `TicketOrchestrationPlanQuery(00001KVHKWNQS)`: previous `blocked_by` plan is resolved by `00001KVHKWNQA` closure; accepted plan recorded now。
- Workspace state:
  - Orchestrator worktree clean at `902b383d`。
  - queued: this Ticket only。
  - inprogress: 0。
  - visible Pods: self + peers only; spawned children 0。
  - no matching implementation branch/worktree。
- Code/resource context:
  - Rust PDK/template resources are now merged from `00001KVHKWNQA`。
  - Component Model runtime and Plugin CLI inspection work are closed and available as implementation context。

IntentPacket:

Intent:
- Add first-party local Plugin authoring CLI commands: `yoi plugin new rust-component-tool <path-or-name>`, `yoi plugin check <path-or-package>`, and `yoi plugin pack <path> [--output <file>]`。
- Make local authoring safe and deterministic without remote scripts, without executing Plugin code during validation, and without mutating workspace enablement config。

Binding decisions / invariants:
- `new` uses embedded templates only; no network, no remote template fetch, no `curl | sh` flow。
- `new` writes only to the requested destination and refuses non-empty destinations unless a narrow explicit safe option is intentionally added。
- Generated Rust Component Tool template should use the current PDK/template resources and current checkout/release dependency policy。
- `check` and `pack` must not execute Plugin code or instantiate components。
- `check` validates directory and `.yoi-plugin` package inputs with bounded diagnostics and stable JSON report shape for `--json`。
- `pack` creates deterministic `.yoi-plugin` output and prints digest/path; `pack --json` returns stable typed output。
- `check` validates manifest/runtime/schema/permission/host API declarations, referenced artifact presence, archive safety, and deterministic digest where applicable。
- `pack` rejects unsafe paths/root escapes and unsupported package shapes; use currently supported archive format/constraints。
- Commands do not mutate enablement/workspace config and do not generate/embed secrets。
- Diagnostics/status language should align with existing `yoi plugin list/show` where possible。
- Do not implement registry publish/install, enabling/disabling config, Plugin execution, Service/Ingress scaffolding, or extra language templates。

Requirements / acceptance criteria:
- `yoi plugin new rust-component-tool ./my-plugin` creates the expected file set from embedded template without network access。
- `yoi plugin check ./my-plugin` validates the generated template and reports next steps/digest/enablement guidance。
- `yoi plugin pack ./my-plugin` creates a `.yoi-plugin` package discoverable by existing Plugin discovery。
- `check` validates existing `.yoi-plugin` archive inputs and rejects unsafe archives。
- `check --json` and `pack --json` provide stable typed reports for tests/agents。
- Tests cover generated file set, overwrite refusal, valid/invalid check paths, missing runtime artifact, unsafe package archive, deterministic pack digest, discoverability by existing Plugin discovery, and JSON report shape。
- Validation includes focused CLI/plugin authoring tests, relevant `cargo check` / `cargo test`, `cargo fmt --check`, `git diff --check`, and `nix build .#yoi` because CLI/resources/package behavior changes。

Escalate if:
- Current package/archive format lacks enough existing helpers and needs a broader package-format decision。
- `check` cannot validate Component world metadata without executing code and needs explicit scope narrowing。
- Deterministic packaging conflicts with current discovery/package reader behavior。
- Embedded template resource packaging/source filtering needs broader product decision。

Next action:
- Record `queued -> inprogress` and commit Ticket records before creating the implementation worktree and spawning Coder。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T05:54:45Z from: queued to: inprogress reason: orchestrator_acceptance_plugin_authoring_cli field: state -->

## State changed

Ticket body/thread, relation metadata, resolved Rust PDK dependency, related closed Tickets, Orchestrator worktree state, visible Pods, existing branch/worktree, and bounded CLI/resource/package context were checked. Dependency `00001KVHKWNQA` is closed, no inprogress/capacity blocker exists, and no missing planning decision was found. Accepting this queued Ticket for implementation before worktree/Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T05:55:31Z -->

## Implementation report

Implementation orchestration start:
- Acceptance records were committed on Orchestrator branch: `d1095f85 ticket: accept plugin authoring cli`。
- Created implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVHKWNQS-plugin-authoring-cli` on branch `impl/00001KVHKWNQS-plugin-authoring-cli` at `d1095f85`。
- Spawned Coder Pod `yoi-coder-00001KVHKWNQS` with write scope limited to the implementation worktree and read-only root scope only for runtime workspace identity。
- Coder task explicitly forbids root/original workspace implementation, merge, Ticket close, and cleanup。

Next action:
- Wait for Coder implementation report, then inspect branch diff/validation evidence and route to Reviewer。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T06:25:39Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVHKWNQS`.

Implementation commit:
- `945ecdf6 plugin: add authoring cli`

Changed areas reported:
- Added `yoi plugin new rust-component-tool <path-or-name>`。
- Added `yoi plugin check <path-or-package> [--json]`。
- Added `yoi plugin pack <path> [--output <file>] [--json]`。
- Added manifest-level materialized package helpers for safe directory/package reading, deterministic digesting, deterministic stored `.yoi-plugin` writing, and symlink/root-escape rejection。
- Reused existing static Plugin inspection for Tool schema / permission / host API declaration checks without Plugin execution。
- Added embedded `plugin.component.wasm` placeholder to Rust component Tool template so generated templates can be checked/packed immediately; docs instruct replacing it before enabling/execution。
- Updated CLI parsing/help and Plugin development docs。
- Added focused tests for generated file set, overwrite refusal, valid directory check, invalid manifest, missing runtime artifact, unsafe package archive, deterministic pack digest, package discoverability, and JSON report shape。

Coder validation reported:
- `cargo fmt`
- `cargo fmt --check`
- `cargo check -p yoi`
- `cargo test -p yoi plugin_cli`
- `cargo test -p yoi-plugin-pdk template`
- `git diff --check`
- CLI smoke:
  - `cargo run -q -p yoi -- plugin new rust-component-tool ... --json`
  - `cargo run -q -p yoi -- plugin check ... --json`
  - `cargo run -q -p yoi -- plugin pack ... --output ... --json`
  - invalid check reports `"status": "rejected"` and exits non-zero。
- `nix build .#yoi --no-link` was attempted but failed due environment/storage: `No space left on device`。
- No Cargo dependency or `Cargo.lock` changes were introduced。

Known risks / deferrals reported:
- Component validation remains metadata/static only; no component instantiation/execution is performed by design。
- Template includes a placeholder artifact solely for deterministic authoring check/pack; docs instruct replacing it before enabling/execution。

Orchestrator evidence checked before review dispatch:
- Implementation worktree is clean on `impl/00001KVHKWNQS-plugin-authoring-cli`。
- HEAD is `945ecdf6`。
- Diff from acceptance `d1095f85..HEAD` is one implementation commit touching 6 files, about 1274 insertions / 24 deletions。
- `git diff --check d1095f85..HEAD` produced no diagnostics。

Next action:
- Dispatch Reviewer for r1 review against Ticket requirements, with focus on non-execution safety, archive/path traversal safety, deterministic pack/digest behavior, placeholder artifact policy, JSON report stability, discoverability, workspace/config non-mutation, CLI UX, tests, and Nix/package validation feasibility。

---

<!-- event: plan author: yoi-orchestrator at: 2026-06-20T06:26:24Z -->

## Plan

Review dispatch:
- Spawned Reviewer Pod `yoi-reviewer-00001KVHKWNQS-r1` against implementation branch `impl/00001KVHKWNQS-plugin-authoring-cli`。
- Review target commit: `945ecdf6 plugin: add authoring cli`。
- Review baseline: `d1095f85`。
- Reviewer task focuses on non-execution safety, archive/path traversal safety, deterministic pack/digest behavior, placeholder artifact policy, JSON report stability, discoverability, workspace/config non-mutation, CLI UX, tests, and Nix/package validation feasibility。
- Reviewer is instructed not to edit source, commit, merge, close the Ticket, or use TicketReview directly; it will report verdict/evidence back to Orchestrator。

---

<!-- event: review author: yoi-reviewer-00001KVHKWNQS-r1 at: 2026-06-20T06:36:52Z status: request_changes -->

## Review: request changes

Verdict: `request_changes`

確認範囲:
- Ticket body/thread / Orchestrator IntentPacket
- Implementation diff: `d1095f85..945ecdf6`
- 主な対象: `crates/yoi/src/plugin_cli.rs`, `crates/yoi/src/main.rs`, `crates/manifest/src/plugin.rs`, `resources/plugin/templates/rust-component-tool/*`, `docs/development/plugin-development.md`, `package.nix`。

Blocking issues:
1. `yoi plugin new` が symlink destination を辿り、物理的な requested destination 外へ書き込める。
   - `materialize_template` は `fs::metadata(destination)` / `fs::read_dir(destination)` を使い、destination symlink を拒否せず `destination.join(relative)` に書く。
   - Reviewer smoke: `linkdest -> target` に対して `plugin new rust-component-tool "$tmp/linkdest"` を実行すると、`target/plugin.toml` が作られた。
   - Ticket invariant: `new` は requested destination にだけ書き、non-empty destination を拒否する。symlink destination はこの境界を破る。

2. Generated placeholder component が `check` で enablement-ready に見える。
   - Template は text placeholder `resources/plugin/templates/rust-component-tool/plugin.component.wasm` を含む。
   - `new` output は next steps で一度 warning するが、fresh generated template に対する `check --json` は `status=active diagnostics=0` になり、generic enablement guidance を出す。
   - Reviewer smoke: generated template check output に placeholder/non-executable warning がなく、`To enable after review...` guidance が出る。
   - Ticket/reviewer contract: check/pack may pass, but output must not imply executable component is ready for enablement if placeholder is not executable。

Required fixes:
- `plugin new` destination symlink を `symlink_metadata` 等で拒否し、symlink write-through を防ぐ。関連する overwrite/destination tests を追加する。
- Generated placeholder artifact を `check` が検出し、bounded diagnostic / next step を出す。Status は `partial` など enablement-ready でない表現にすることを優先する。少なくとも `active diagnostics=0` と enablement guidance の組み合わせは避ける。
- `check --json` and human output tests should cover generated template placeholder warning/status。

Non-blocking concerns:
- `nix build .#yoi --no-link` は environment storage (`No space left on device`) で失敗。source-filter/resource-packaging issue は見えていないが、最終 Orchestrator validation で再試行する。
- `pack --json` digest is nested under `package.digest`, not top-level. Acceptable if documented/stable, but consider documenting shape。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check d1095f85..HEAD`: passed
- `cargo check -p yoi`: passed
- `cargo test -p yoi plugin_cli`: passed (`15 tests`)
- `cargo test -p yoi-plugin-pdk template`: passed
- CLI smoke for `new`, `check --json`, `pack --json`, packed package check/list discovery, invalid check rejected: passed except blocker findings above。
- `nix build .#yoi --no-link`: failed due `No space left on device`。

Worktree status at review end: clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T06:46:09Z -->

## Implementation report

Coder r1-fix report received from `yoi-coder-00001KVHKWNQS`.

New fix commit:
- `699db538 plugin: harden authoring checks`

Branch commits now:
- `699db538 plugin: harden authoring checks`
- `945ecdf6 plugin: add authoring cli`

Fix summary:
- `plugin new` rejects an existing destination symlink via `symlink_metadata` before writes。
- Added regression test ensuring symlink destination is refused and target is not written。
- `plugin check` detects embedded generated placeholder `plugin.component.wasm`。
- Fresh generated templates report `status: "partial"` instead of `active`。
- JSON/human diagnostics include bounded placeholder warning。
- Enablement guidance no longer says a package is ready to enable while placeholder artifact remains。
- `partial` checks exit successfully; `rejected` checks remain non-zero。
- Docs updated for symlink refusal and placeholder/partial behavior。

Coder validation reported as passing:
- `cargo fmt --check`
- `git diff --check d1095f85..HEAD`
- `cargo check -p yoi`
- `cargo test -p yoi plugin_cli`
- `cargo test -p yoi-plugin-pdk template`
- CLI smoke:
  - `plugin new ... --json`
  - `plugin check <generated> --json` => `partial` with placeholder diagnostic。
  - `plugin pack ... --json` => `packed`。
  - `plugin check <packed .yoi-plugin> --json` => `partial`。
  - copied packed archive into `.yoi/plugins` and confirmed `plugin list --workspace ... --json` discovers it。
  - invalid check after removing runtime artifact => `rejected` and non-zero。
- Freed only child-worktree Cargo build artifacts with `cargo clean` and reran `nix build .#yoi --no-link`; it completed successfully。

Orchestrator evidence checked before r2 review:
- Implementation worktree is clean。
- HEAD is `699db538`。
- Fix diff from `945ecdf6..HEAD` touches `crates/yoi/src/plugin_cli.rs` and `docs/development/plugin-development.md`, about 124 insertions / 34 deletions。
- Overall `git diff --check d1095f85..HEAD` produced no diagnostics。

Next action:
- Route back to Reviewer for r2 verification of symlink destination refusal, placeholder partial/check behavior, and final acceptance criteria。

---

<!-- event: review author: yoi-reviewer-00001KVHKWNQS-r1 at: 2026-06-20T06:50:40Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket body/thread / Orchestrator IntentPacket
- r2 fix diff: `945ecdf6..699db538`
- 全体実装 diff: `d1095f85..699db538`
- 主な対象: `crates/yoi/src/plugin_cli.rs`, `docs/development/plugin-development.md`, and prior `new` / `check` / `pack` implementation surface。

Previous blocker verification:
1. `plugin new` symlink destination write-through
   - `materialize_template` now uses `fs::symlink_metadata` and rejects destination symlink before writing。
   - Regression test `plugin_new_refuses_symlink_destination_without_following_it` verifies refusal and no write-through。
   - CLI smoke confirmed symlink destination is rejected and target directory does not receive `plugin.toml`。

2. Placeholder component reported as enablement-ready
   - `check` now compares runtime component bytes against embedded template placeholder and adds bounded `placeholder` diagnostic。
   - Generated template now reports `status=partial` when static validation is otherwise clean but placeholder remains。
   - Enablement guidance is suppressed for `partial` and says the package is not ready to enable。
   - CLI smoke confirmed generated directory and packed `.yoi-plugin` both report `partial` with placeholder diagnostic; `partial` exits successfully, while invalid/missing paths remain non-zero rejected。

Blocking issues: none。

Non-blocking concerns / follow-ups:
- None required for this Ticket. The hardening keeps status vocabulary aligned with existing Plugin inspection (`active` / `partial` / `rejected`)。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check d1095f85..HEAD`: passed
- `cargo check -p yoi`: passed
- `cargo test -p yoi plugin_cli`: passed (`16 tests`)
- `cargo test -p yoi-plugin-pdk template`: passed (`2 template tests`)
- CLI smoke after `cargo build -p yoi`: passed for `new`, generated `check --json partial`, human `[partial]`, `pack --json`, packed package `check --json partial`, invalid check rejected/non-zero, symlink destination refusal, and `plugin list --workspace ... --json` discoverability as `disabled`。
- `nix build .#yoi --no-link`: passed

Worktree status at review end: source tree clean。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-20T06:54:35Z -->

## Implementation report

Merge and final validation report:

Merged implementation branch into Orchestrator branch:
- Merge commit: `87704ad4 merge: plugin authoring cli`
- Implementation commits included:
  - `945ecdf6 plugin: add authoring cli`
  - `699db538 plugin: harden authoring checks`

Reviewer outcome:
- r1 requested changes for symlink destination write-through and placeholder component being reported as enablement-ready。
- Coder added symlink destination refusal, placeholder detection, `partial` status / diagnostics, and tests。
- r2 approved with no blocking issues。

Orchestrator validation after merge:
- Passed before Nix:
  - `cargo fmt --check`
  - `git diff --check HEAD^1..HEAD`
  - `cargo check -p yoi`
  - `cargo test -p yoi plugin_cli`
  - `cargo test -p yoi-plugin-pdk template`
- Initial `nix build .#yoi --no-link` failed with environment storage exhaustion while building `aws-lc-sys` (`No space left on device`), not a source/package diagnostic。
- Orchestrator freed only Orchestrator-worktree Cargo build artifacts with `cargo clean` (`43.3GiB`) and reran:
  - `nix build .#yoi --no-link`: passed。
  - `nix path-info -S .#yoi`: `112260512`。

Validation log for first grouped run:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-Q0KE3A.log`

Final state:
- Orchestrator worktree clean at `87704ad4` after successful Nix validation。
- Implementation worktree remains available for cleanup after Ticket completion records are committed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-20T06:54:45Z from: inprogress to: done reason: merged_reviewed_validated field: state -->

## State changed

Implementation was merged into Orchestrator branch at `87704ad4`, r2 review approved, and final Orchestrator validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo check -p yoi`, focused `yoi plugin_cli` / `yoi-plugin-pdk template` tests, and `nix build .#yoi --no-link` after freeing Orchestrator worktree build artifacts.

---
