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
