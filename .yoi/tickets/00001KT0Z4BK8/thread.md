<!-- event: create author: tickets.sh at: 2026-06-01T06:49:53Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-01T06:50:33Z -->

## Decision

Distribution direction from user discussion:

- Initial plugin packages should be single-file archives placed in user or workspace plugin stores, such as `~/.config/insomnia/plugins/` and `./.insomnia/plugins/`.
- Package presence is discovery only. Tool/Hook registration, WASM initialization, or MCP process startup requires explicit manifest/profile enablement and capability grant resolution.
- The package should contain a root `plugin.toml` and optional runtime assets such as `module.wasm`, schemas, README, and license files.
- User and workspace stores map to source/trust categories. `user:<id>` and `project:<id>` are distinct; ambiguous selectors should fail closed.
- Workspace packages are repository-provided and must be treated conservatively, especially for executable runtimes and MCP/process-spawn behavior.


---

<!-- event: decision author: hare at: 2026-06-13T15:29:21Z -->

## Decision

決定:
- `pod::feature` は API / contribution substrate として扱い、Plugin や MCP の権限管理を担わせない。
- Plugin は `pod::feature` をユーザー向け package/config/runtime 形式で使わせる層であり、Plugin permission / trust policy は Plugin layer で定義する。
- MCP は `pod::feature` 上に protocol-backed integration layer を構築するが、MCP server enablement / command-env-secret policy / trust boundary / MCP-specific permission は MCP layer が独自に持つ。
- MCP local stdio server の OS-level side effects は Yoi feature authority では制御できないため、feature-layer authority / grant を MCP や Plugin の permission model に流用しない。

反映:
- `00001KTR81P9X` は authority ではなく provider lifecycle / dynamic contribution / normal ToolRegistry path / untrusted normalization に絞る。
- `00001KTR82RB7` は MCP 固有の explicit config と trust model を持つ。
- `00001KSXRQ4G8` と `00001KT0Z4BK8` は Plugin permission を Plugin layer として扱い、MCP を初期 Plugin packaging/runtime から分離する。


---

<!-- event: decision author: hare at: 2026-06-14T14:34:33Z -->

## Decision

決定:
- Plugin discovery stores, source-qualified identity, discovery/enablement separation, Pod-startup initialization timing, restore behavior, and MCP boundaryをこの `00001KT0Z4BK8` の安定 contract として統合した。
- User Plugin package store は `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/` とし、`~/.config/yoi/plugins/` は使わない。
- Package presence は discovery only であり、実行・登録・runtime initialization は explicit enablement 後の Pod startup に限定する。
- Restore は metadata snapshot の enabled Plugin plan を正本とし、fresh discovery で silent upgrade しない。


---

<!-- event: intake_summary author: hare at: 2026-06-14T14:53:35Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-14T14:53:35Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T14:54:52Z from: ready to: planning reason: panel_return_to_planning field: state -->

## State changed

Panel returned ready Ticket 00001KT0Z4BK8 to planning for requirements sync. This is not Queue routing and must not start implementation.

## User refinement instruction

前何回かに渡って実装されたlatencyとかE2Eとかの話なんだけど


---

<!-- event: comment author: intake at: 2026-06-14T14:56:35Z -->

## Comment

Intake 確認:
- Ticket `00001KT0Z4BK8` の body / thread / artifacts を確認した。この Ticket の現在の主題は Plugin distribution package format / discovery / enablement boundary であり、直近の user refinement「前何回かに渡って実装されたlatencyとかE2Eとかの話なんだけど」は、この Ticket の plugin package 要件とは直接対応しない。
- 関連しそうな既存 Ticket として `00001KV0723PC`（Panel Quit latency）、`00001KSKBP9YG`（E2E harness）、`00001KV0TJVN5`（E2E binary/env isolation）、`00001KV0YK5S0`（E2E tmp/runtime isolation）、`00001KV10SN02`（E2E critical-path coverage）を確認した。いずれも closed で、latency/E2E の実装・検証・残差整理はそちらの系譜に記録されている。
- 現時点では、Plugin package Ticket の readiness / requirements を変更する根拠は不足している。ユーザーに、(a) `00001KT0Z4BK8` に latency/E2E 由来の検証要件を追加したいのか、(b) latency/E2E 系の既存 Ticket に追記したいのか、(c) 新しい concrete follow-up Ticket が必要なのか、を確認する。

状態:
- このコメントは要件同期の保留記録であり、queue / implementation routing ではない。
- duplicate Ticket は作成していない。

---

<!-- event: comment author: intake at: 2026-06-14T15:12:07Z -->

## Comment

ユーザー確認の要点:
- ユーザーは latency / mouse / E2E の経緯説明ではなく、最終的な evidence status を確認している。
- Ticket 記録上、`00001KV0723PC` の元の Panel Quit latency 修正は post-merge measurement で「still present / not proven fixed」と記録されており、ユーザー観測の latency 改善としては最終的に認められていない。
- `00001KV072V89` の元の mouse selection 修正も post-merge user verification で「マウス選択全く効かない」と記録され、元の done decision は insufficient とされている。
- 後続 E2E は fixture/PTY 上で mouse click/wheel と quit pending-barrier regression を pass させたが、これは user の実環境・実インストール経路で latency が解消したこと、または mouse 操作が実際に動作したことの human/live confirmation ではない。
- `00001KT0Z4BK8` 自体の Plugin package requirements には現時点で反映しない。必要なら別 follow-up Ticket として「E2E 結果と user-visible validation gap の整理 / merge gate 明文化」を扱う。

---

<!-- event: intake_summary author: hare at: 2026-06-14T15:37:24Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-14T15:37:24Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T15:40:15Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:40:52Z -->

## Decision

Routing decision: implementation_ready_but_waiting_capacity

Reason:
- Ticket body / thread / artifacts、relation、OrchestrationPlan、Orchestrator workspace state を確認した。Plugin package / discovery / enablement boundary の design work item として要件・受け入れ条件・non-goals・invariants は十分に具体化されている。
- blocking relation / OrchestrationPlan blocker はない。
- Plugin package work は現在 active な Panel/TUI implementation と source surface が大きく重ならないため、設計上の conflict blocker ではない。
- ただし現在 `00001KTFY8V80` と `00001KV09WYC6` の2件が inprogress で Coder Pod running。Reviewer follow-up と integration capacity も未使用ではなく、さらに queued Panel/TUI work 2件を待機させている。
- 現時点では追加 Coder Pod を spawn せず、active Coder のいずれかが implementation report を返して review/integration 見通しが立ってから acceptance する。

Evidence checked:
- Ticket body/thread: Plugin package design requirements、過去の Plugin/MCP/feature-layer decision、`planning -> ready`、Panel `ready -> queued` を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、queue commit `4be6c966` 上。
- Visible Pods: `yoi-coder-00001KTFY8V80` と `yoi-coder-00001KV09WYC6` が running。

Next action:
- 先行 inprogress Ticket の少なくとも1件が implementation report / review stage に進み、Coder capacity が空いた時点で再確認し、unblocked なら `queued -> inprogress` acceptance と dedicated worktree 作成へ進む。
- planning return ではなく queued のまま waiting とする。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:49:34Z -->

## Decision

Routing decision: implementation_ready

Reason:
- 先行 Coder のうち `00001KV09WYC6` が implementation report を返し review stage に入ったため、Plugin work 用の Coder capacity を再評価した。
- Ticket body / thread / relations / orchestration plan / Orchestrator workspace state を再確認した。blocking relation はなく、既存 waiting note は capacity 起因であり、現在は1件分の Coder capacity を空けられる。
- 本 Ticket は Plugin package / discovery / enablement boundary の design/documentation work が主で、active Panel/TUI implementation と source surface が大きく重ならない。
- Plugin/MCP/feature-layer authority boundary に関する prior decisions は Ticket thread に記録済みで、残る不確実性は proposal の構成・記述・必要最小限の config shape 調査に閉じている。

Evidence checked:
- Ticket body / thread: package format、store/source mapping、discovery vs enablement、manifest semantics、runtime-specific notes、cache/pinning、diagnostics、prior Plugin/MCP/feature-layer decisions を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: capacity waiting note 1件のみ。blocking/conflict record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`80a9e40d` 上。
- Active Pods: `00001KTFY8V80` coder running、`00001KV09WYC6` reviewer running。
- Bounded code/doc map: Plugin docs は未作成。関連 candidate は `docs/design/*`, `crates/manifest/src/{config,profile}.rs`, `crates/pod/src/feature.rs`, `crates/pod/src/hook.rs`。

IntentPacket:

Intent:
- `.yoi-plugin` package distribution/discovery/enablement boundary の durable design proposal を repository に追加し、後続 implementation Ticket を独立して切れる状態にする。

Binding decisions / invariants:
- Package presence in user/workspace plugin stores is discovery only; registration, WASM init, Hooks/Tools contribution, process/server startup, and MCP server launch require explicit enablement and grants.
- Source-qualified identity is required: `user:<id>`, `project:<id>`, `builtin:<id>` are distinct; ambiguous unqualified IDs fail closed.
- Plugin permission declarations are requests, not grants. Effective grants are Plugin-layer policy plus existing manifest/profile/scope/tool/web/secret/runtime allowlists.
- Do not model Plugin permissions with `pod::feature` HostAuthority/grant concepts.
- MCP remains a separate feature-backed integration and is out of initial Plugin packaging/runtime unless future Ticket explicitly approves a bridge.
- Archive handling must reject path traversal and unsafe layout, use bounded extraction, compute deterministic digest, and materialize into digest-keyed cache before runtime initialization.
- Restore should use resolved manifest/session metadata for enabled Plugin plan; fresh discovery must not silently upgrade a restored Pod.

Requirements / acceptance criteria:
- Repository contains a documented Plugin distribution/package proposal covering `.yoi-plugin` archive structure, root `plugin.toml`, assets, user/workspace/builtin stores, source/trust mapping, identity collision rules, discovery vs enablement, manifest fields, archive safety, cache/digest/pinning, diagnostics, and runtime-specific notes for declarative hooks and WASM.
- Proposal explicitly states store placement is discovery only, not execution or registration.
- Proposal distinguishes Plugin permission request/grant model from `pod::feature` authority concepts.
- Proposal calls out MCP as separate and out of initial Plugin packaging.
- Follow-up implementation cuts are clear for manifest/profile enablement, package discovery, archive validation/cache, Plugin permission policy, WASM packaging, and any future MCP/plugin bridge.

Implementation latitude:
- Primary deliverable may be a design doc plus minimal cross-references; code changes are optional and should stay within safe internal boundaries.
- Coder may choose exact doc path/name consistent with existing docs organization.
- If proposing config shape, prefer illustrative schemas over broad runtime implementation unless obviously small and safe.

Escalate if:
- A real runtime implementation becomes necessary to satisfy the Ticket.
- Plugin package design would require changing Profile/manifest authority semantics, Pod restore semantics, secret handling, or MCP enablement model.
- The proposal would imply workspace package execution or silent restore upgrades.

Validation:
- `cargo fmt --check` if code or Rust doc tests are touched.
- `git diff --check` always.
- If only Markdown docs are touched, focused validation may be `git diff --check` plus link/path sanity review.

Current code/doc map:
- Likely doc destination: `docs/design/`.
- Related architecture candidates: `crates/manifest/src/config.rs`, `crates/manifest/src/profile.rs`, `crates/pod/src/feature.rs`, `crates/pod/src/hook.rs`.

Critical risks / reviewer focus:
- Discovery vs enablement separation.
- Plugin permission requests vs grants.
- MCP separation.
- Source identity collision/fail-closed behavior.
- Archive safety and digest/cache semantics.
- Restore/fresh discovery no silent upgrade invariant。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T15:49:39Z from: queued to: inprogress reason: orchestrator_acceptance_after_capacity_release field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / orchestration-plan blocker はなく、capacity waiting reason は解消した。implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KT0Z4BK8 at: 2026-06-14T15:53:15Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KT0Z4BK8`:

Commit:
- `3c674a70 docs: propose plugin package distribution`

Changed files:
- `docs/design/plugin-packages.md`
  - Added the `.yoi-plugin` package/discovery/enablement design proposal.
- `docs/README.md`
  - Added the new design doc to the maintained documentation reading order.

Design choices covered:
- `.yoi-plugin` as a constrained single-file archive with required root `plugin.toml`.
- Suggested package layout for `module.wasm`, declarative hooks, schemas, README/license, and assets.
- User/workspace/builtin plugin stores and source-qualified identity: `user:<id>`, `project:<id>`, `builtin:<id>`.
- Explicit discovery-vs-enablement separation.
- Store presence under `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/` or `<workspace>/.yoi/plugins/` is discovery only, never registration/execution.
- Plugin permission declarations are requests, not grants.
- Plugin permission policy is explicitly separate from `pod::feature` HostAuthority/grant concepts.
- Archive validation, traversal rejection, bounded extraction, deterministic digest, digest-keyed cache materialization, and optional pin/lock behavior.
- Restore uses resolved enabled-plugin plan/digest metadata; fresh discovery must not silently upgrade a restored Pod.
- Diagnostics guidance without secret leakage.
- Runtime notes for declarative hooks and WASM.
- MCP is explicitly out of initial Plugin packaging/runtime and requires a future separate bridge Ticket.
- Follow-up implementation cuts are listed for enablement schema, discovery, validation/cache, permission policy, declarative hooks, WASM, locks, and future MCP bridge.

Validation reported by coder:
- Passed: `git diff --check`.
- Link/path sanity review:
  - Verified `docs/README.md` references existing `docs/design/plugin-packages.md`.
  - Verified touched Markdown files have trailing newlines and no tab characters.
- Post-commit check:
  - `git status --short --branch` showed clean branch `impl/00001KT0Z4BK8-plugin-package-discovery`.
  - `git diff --check HEAD~1 HEAD` passed with no whitespace errors.
- No Rust/code changes were made, so `cargo fmt` / `cargo test` were not run.

Repository status:
- Child implementation worktree clean after commit.

Residual risks / blockers:
- This is intentionally a design proposal only. Exact manifest/Profile schema, lock-file format, archive limits, cache path, and WASM ABI details remain for follow-up implementation Tickets.

---

<!-- event: review author: yoi-reviewer-00001KT0Z4BK8 at: 2026-06-14T15:56:22Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- Child worktree/branch:
  - `/home/hare/Projects/yoi/.worktree/00001KT0Z4BK8-plugin-package-discovery`
  - `impl/00001KT0Z4BK8-plugin-package-discovery`
- HEAD: `3c674a70512ca31b5745d901959c04442c1695d0`
- Base merge point: `d73f748ee8d2e25217cafe3754eb9fa8870ddbed`
- Diff `d73f748e..HEAD` inspected:
  - added `docs/design/plugin-packages.md`
  - updated `docs/README.md`
- Ticket intent/acceptance context reviewed from the child worktree Ticket record.

Acceptance criteria review:
- `.yoi-plugin` archive structure and required root `plugin.toml` are documented.
- Packaged assets/layout are covered, including optional WASM module, hooks, schemas, README/license, and `assets/**`.
- Stores and source/trust mapping are covered for `builtin:<id>`, `user:<id>`, and `project:<id>`.
- Package presence in user/workspace stores is clearly discovery only, not execution/registration.
- Source-qualified identity, ambiguous-id fail-closed behavior, and collision handling are covered.
- Discovery vs enablement and restore/no-silent-upgrade behavior are explicit.
- Manifest/Profile enablement shape is illustrative and appropriately deferred.
- Plugin permission declarations are requests, not grants; effective grants are tied to Plugin-layer policy plus existing manifest/profile/tool/scope/web/secret/runtime authority layers.
- The document avoids using `pod::feature` HostAuthority/grant concepts as Plugin permission/security model.
- Archive safety covers traversal rejection, unsafe file types, bounded extraction, deterministic digest, digest-keyed cache, and manifest path validation.
- Diagnostics guidance covers attribution, bounded output, and no secret leakage.
- Runtime notes cover declarative hooks, WASM initialization from digest cache, host limits, and ToolRegistry/permission checks.
- MCP is explicitly separate and out of the initial Plugin package runtime.
- Follow-up implementation cuts are clear and separable.
- `docs/README.md` cross-reference is appropriate and remains Why/design-oriented.

Validation performed:
- Passed: `git diff --check d73f748e..HEAD`
- Passed: `git diff --check HEAD~1 HEAD`
- README-listed relative doc target existence checked with shell commands.
- Manual Markdown/design boundary review completed.

Validation not run:
- No cargo commands because the change is documentation-only.
- A Python-based link check could not run because `python3` is unavailable; shell existence checks were used instead.

Conclusion:
- Approved. No blocking concern remains.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-14T15:56:45Z -->

## Implementation report

Integration report:

Merged implementation branch into orchestration branch:
- branch: `impl/00001KT0Z4BK8-plugin-package-discovery`
- implementation commit: `3c674a70 docs: propose plugin package distribution`
- merge commit: `2b9dae48 merge: plugin package design`

Reviewer result:
- `approve` recorded from `yoi-reviewer-00001KT0Z4BK8`.

Validation in Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`:
- Passed: `git diff --check`
- Passed: `test -f docs/design/plugin-packages.md`
- Passed: `grep -n 'plugin-packages.md' docs/README.md`

Cargo validation:
- Not run because the merged change is documentation-only.

Notes:
- The proposal is intentionally design-only. Exact manifest/Profile schema, lock-file format, archive limits, cache path, and WASM ABI remain follow-up implementation work.
- Orchestrator worktree is clean after validation.

Cleanup planned:
- Stop related coder/reviewer Pods.
- Remove only child implementation worktree/branch for this Ticket.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T15:56:45Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Reviewer approved, documentation/design implementation branch merged into the orchestration branch, and documentation-focused validation passed in the Orchestrator worktree. Marking Ticket done in the orchestration branch.

---

<!-- event: state_changed author: hare at: 2026-06-15T06:33:50Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-15T06:33:50Z status: closed -->

## 完了

Ticket `00001KT0Z4BK8` (`Plugin distribution package format and discovery`) はすでに `state: done` に到達していたため、workspace Panel から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
