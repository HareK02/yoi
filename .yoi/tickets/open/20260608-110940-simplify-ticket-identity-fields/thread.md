<!-- event: create author: LocalTicketBackend at: 2026-06-08T11:09:40Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-08T11:15:09Z -->

## Decision

## Scope expansion: simplify core Ticket frontmatter, not only identity

Expand this Ticket beyond identity cleanup. The same simplification pass should also review and reduce the core Ticket frontmatter shape.

### Unify `status` and `workflow_state`

The current split between local `status` and `workflow_state` creates an invalid two-axis state model. Combinations such as `status: closed` with `workflow_state: inprogress` are not meaningful.

Direction:

- Use one canonical lifecycle state field.
  - Final name can be `state` or retained as `workflow_state`, but there should not be both `status` and `workflow_state` as independent current fields.
- Treat `open` as a derived range of states rather than a stored independent status.
  - Example: `open = planning | ready | queued | inprogress | done`.
  - Terminal closed state should be represented by the same lifecycle model, e.g. `closed` or equivalent.
- Decide whether `done` and `closed` remain distinct.
  - `done`: implementation/review/merge complete, close/resolution may still be pending.
  - `closed`: resolution recorded and Ticket lifecycle ended.
- Revisit or remove the `pending` bucket/status. If a concept is still needed, define it as a lifecycle state such as `deferred` / `parked`, not as a second axis.
- Update path layout expectations if needed.
  - Directory buckets may be derived caches or UI grouping, but frontmatter should not duplicate contradictory state.

### Remove `kind`

`kind` currently behaves like a single typed category but is a freeform string with unclear management and little semantic authority. Most Tickets are work items and can be described by their title/body.

Direction:

- Remove `kind` from the required/current schema unless a small typed enum with clear semantics is explicitly justified.
- Do not keep required freeform `kind`.
- If the title follows a convention such as `area: verb object`, it already communicates the work class better than an unconstrained `kind: task` field.

### Remove `labels`

`labels` are useful for search, but without a registry/canonical vocabulary they create inconsistent taxonomy and can be confused with state, relation, risk, component, or priority.

Direction:

- Remove `labels` from the core required/current schema in this simplification pass.
- Do not use labels to represent state, dependency, blockers, risk, priority, or ownership.
- If tag-like search is needed later, add it as a separate feature with a project-managed registry/canonicalization model.
- Prefer descriptive titles and body content for search. A title convention such as `component: concise action` can carry much of the useful categorization without an unmanaged labels field.

### Resulting target direction

The target core Ticket identity/frontmatter may be as small as:

```yaml
id: 20260608-103842   # or path-derived primary key
title: component: concise action
state: planning
created_at: ...
updated_at: ...
```

Additional fields should justify a concrete behavior. Search/display hints without management authority should be removed or moved to separately designed features.

---

<!-- event: plan author: intake at: 2026-06-09T00:16:54Z -->

## Plan

## Intake assessment: requirements sync needed

この Ticket は既存 identity simplification に加えて、`status` / `workflow_state` / `kind` / `labels` を含む core Ticket frontmatter simplification へ scope が拡張されている。方向性は明確だが、実装前に schema/API/UX と移行方針として固定すべき判断が残っているため、現時点では `workflow_state: planning` のまま requirements sync が必要。

### 既に決まっていること

- canonical Ticket identity は title-derived slug words を含めない timestamp/counter primary key へ寄せる。
- `slug` は required/current frontmatter および canonical lookup key から外す。
- agents / Orchestrator は ID/title だけから意味を推測せず、routing/実装前に `TicketShow` の body/thread/artifacts を読む。
- core frontmatter simplification は identity だけでなく lifecycle/kind/labels も対象に含める。
- `kind` は required freeform field として残さない方向。
- `labels` は unmanaged taxonomy として core required/current schema から外す方向。

### 実装前に固定すべき open questions

1. lifecycle field の最終名を `state` に変更するか、既存名 `workflow_state` を保持して single lifecycle field にするか。
2. `done` と `closed` を distinct lifecycle states として残すか、close/resolution の表現をどう統合するか。
3. `pending` bucket/status を完全に廃止するか、`deferred` / `parked` などの lifecycle state として置き換えるか。
4. canonical ID を frontmatter に `id` として保持するか、directory name から derive するか。
5. legacy slug/path lookup を migration-only としてどこまで読むか。unreleased local data として一括 migration し、current schema では permanent alias を作らない方針でよいか。
6. `kind` / `labels` removal をこの Ticket の実装 scope に含めて同時 migration するか、identity/lifecycle の破壊的変更と分けるか。

### Risk flags / reviewer focus

- `ticket-schema`
- `storage-migration`
- `workflow-state`
- `panel-actions`
- `orchestrator-routing`
- `backward-compatibility`
- `typed-tool-api`

Reviewer は diff だけでなく、CLI / typed Ticket tools / Panel / Orchestrator routing / local role-session claims / future relation metadata が、slug や two-axis state を authority として残していないことを確認する必要がある。

### 次の Intake action

上記 open questions に user/maintainer decision が入れば、Ticket を `implementation_ready` として `ready` にできる。未回答のまま進める場合は、先に design/spike として routing し、実装 Ticket とは分けるのが安全。

---

<!-- event: decision author: intake at: 2026-06-09T02:13:15Z -->

## Decision

## User decision: target Ticket schema and storage shape

User/maintainer decision for this Ticket:

- Lifecycle field は `state` に統一する。`status` と `workflow_state` の二軸 current state は廃止する。
- `done` と `closed` は一旦 distinct states として分離したままにする。
  - `done`: 実装・review・merge 等は完了しているが、resolution/close 処理前の状態。
  - `closed`: resolution が記録され、Ticket lifecycle が終了した状態。
- `pending` は不要。pending bucket/status は current model から削除する。
- filesystem layout は状態 bucket を持たず、すべて平坦に `.yoi/tickets/<ticket-id>/` に置く。
  - open/pending/closed directory bucket は authority ではなくなり、current layout からも外す。
- canonical Ticket ID は directory name から derive し、frontmatter には `id` を重複保存しない。
- `kind` / `labels` 等の core frontmatter 削除もこの Ticket の scope に含める。

### Updated target direction

Current frontmatter は少なくとも以下へ縮小する方向で実装する。

```yaml
title: component: concise action
state: planning
created_at: ...
updated_at: ...
```

Canonical identity は `.yoi/tickets/<ticket-id>/` の `<ticket-id>`。`slug`、frontmatter `id`、directory bucket、freeform `kind`、unmanaged `labels` は current schema authority から外す。

### Implementation latitude

- Timestamp/counter ID の exact format、collision suffix、高解像度 timestamp のどれを使うかは、opaque ID・安定 lookup・testability を満たす範囲で実装側が選べる。
- Existing local records は新 layout/schema に migration する。unreleased local data として扱い、明示的に必要にならない限り permanent slug alias や旧 bucket layout の長期互換層は作らない。
- `priority`、`action_required`、`attention_required`、`queued_at` 等の周辺 field は、削除・置換・維持の判断を「具体的な現在動作を持つか」で監査し、core identity/lifecycle simplification を歪めない範囲で扱う。

### Reviewer focus

Reviewer は、CLI / typed Ticket tools / Panel / role-session claims / Orchestrator guidance / relation metadata が、旧 `slug`、frontmatter `id`、`status`/`workflow_state` 二軸、open/pending/closed bucket を authority として残していないことを重点確認する。

---

<!-- event: intake_summary author: intake at: 2026-06-09T02:13:23Z -->

## Intake summary

Ticket schema simplification の binding decisions が揃った。Lifecycle は `state` に統一し、`done` と `closed` は一旦分離、`pending` は廃止する。Filesystem layout は `.yoi/tickets/<ticket-id>/` の平坦構造にし、canonical ID は directory name から derive して frontmatter に重複保存しない。`slug`、frontmatter `id`、freeform `kind`、unmanaged `labels`、open/pending/closed bucket、`status`/`workflow_state` 二軸 state は current schema authority から外す。実装前に Orchestrator/Coder は CLI・typed tools・Panel・role-session claims・Orchestrator guidance・relation metadata の旧 identity/state 依存を監査し、local records を新 layout/schema に migrate すること。

---

<!-- event: state_changed author: intake at: 2026-06-09T02:13:23Z from: planning to: ready reason: planning_ready field: workflow_state -->

## State changed

必要な schema/lifecycle/storage layout の判断が user decision として記録されたため、Orchestrator が実装 routing できる状態になった。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T02:13:30Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-09T02:14:31Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket body/thread, binding user decisions from Intake, current workspace state, and visible worktree/Pod state. No active implementation worktree remains. This is high-risk/broad Ticket schema/storage migration, but the required decisions are explicit enough to proceed with strict implementation invariants, migration evidence, and reviewer focus rather than returning to planning.

---

<!-- event: decision author: orchestrator at: 2026-06-09T02:14:31Z -->

## Decision

Routing decision: implementation_ready

Evidence checked:
- Ticket body, expanded scope decision, Intake assessment, user binding decision, Intake summary, and queued event.
- Current workspace state: clean after committing queued event.
- Worktree list: only main workspace, no active implementation worktree.
- Visible Pods: no active sibling coder/reviewer from previous work remains.

Reason:
- This is a broad and high-risk schema/storage migration, but the Ticket now contains explicit binding decisions for identity, lifecycle, path layout, and core frontmatter simplification.
- Returning to planning would need a concrete missing decision; after reading the Ticket/thread, the main open questions have user decisions.
- Proceed with a strict audit-first implementation and reviewer focus.

IntentPacket:

Intent:
- Simplify current Ticket identity/storage/frontmatter around path-derived opaque timestamp/counter IDs, a single lifecycle `state`, and minimal current frontmatter, while migrating local Ticket records and updating all current Ticket surfaces.

Binding decisions / invariants:
- Canonical Ticket identity is the directory name under flat `.yoi/tickets/<ticket-id>/`.
- The canonical ID must be opaque timestamp/counter style and must not include title/slug words.
- Frontmatter must not duplicate canonical identity as current `id`.
- `slug` is not a required/current field and is not a canonical lookup key.
- `status` and `workflow_state` must not remain independent current state axes; use one canonical lifecycle field named `state`.
- `done` and `closed` remain distinct states for now:
  - `done`: implementation/review/merge complete but resolution/close handling not yet final;
  - `closed`: resolution recorded and Ticket lifecycle ended.
- `pending` bucket/status is removed from the current model.
- Current filesystem layout is flat `.yoi/tickets/<ticket-id>/`, not `.yoi/tickets/{open,pending,closed}/...`.
- `kind` and unmanaged `labels` are removed from current required/core frontmatter.
- New Ticket records should use minimal current frontmatter, approximately `title`, `state`, `created_at`, `updated_at` plus only fields with concrete current behavior that survive audit.
- Existing local `.yoi/tickets` records must be migrated to the new layout/schema.
- Permanent slug aliases / long-term old bucket compatibility should not be added unless a concrete current requirement is discovered; this is unreleased local data.
- Exact canonical ID lookup remains; title/search UX may be list/search-oriented but must not make slug canonical.
- Panel actions, role/session claims, tool/API outputs, Ticket doctor, CLI, Orchestrator guidance, and future relation/orchestration-plan references must use canonical IDs, not slug/path-derived title words.
- Agents/Orchestrator guidance must reinforce reading `TicketShow` body/thread/artifacts before routing/implementation; do not infer requirements from ID/title alone.
- Do not implement full typed Ticket relation metadata here.
- Do not remove `title`.
- Do not change Pod protocol/history/session semantics except where Ticket storage references are legitimately updated.

Implementation latitude:
- Coder may choose timestamp collision strategy (counter suffix or higher-resolution timestamp) if deterministic/testable and ID remains opaque.
- Coder may keep tightly scoped migration-only parser/lookup helpers if needed to read old local records during migration, but current writers/tools/docs must emit/use the new schema/layout.
- Coder may split implementation internally into commits if useful, but should report a coherent final branch state.
- Coder should stop and report if the change proves too large to keep the repository buildable or if a hidden external compatibility requirement requires long-term legacy aliases.

Reviewer focus:
- Verify there is no old authoritative dependency on slug, frontmatter id, open/pending/closed buckets, status/workflow_state two-axis state, freeform kind, or unmanaged labels in current CLI/tools/panel/backend/claims/docs.
- Verify local record migration is complete and `ticket doctor` validates the new flat layout/schema.
- Verify exact lookup and user-facing display remain usable with `id + title`.
- Verify close/resolution semantics distinguish `done` vs `closed` as decided.
- Verify legacy compatibility is migration-only and does not reintroduce permanent slug authority.

Escalate if:
- Implementation requires a broad backward-compatibility layer for old slug/bucket layouts.
- The change cannot keep the repo in a buildable state while migrating records.
- Existing code depends on old directory buckets in a way that requires a larger storage-provider redesign.
- Tool schema changes would require external plugin/capability redesign beyond Ticket built-in tools.

Validation:
- Focused Ticket backend/parser/writer/doctor/create/list/show/query tests for flat layout, path-derived ID, `state`, collision handling, and migrated records.
- Focused tool schema/output tests for TicketCreate/List/Show/WorkflowState/Close/Status-equivalent behavior.
- Focused panel/role-session/orchestrator guidance tests where surfaces changed.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor` or the new equivalent if command behavior changes.
- `cargo check --workspace`.
- `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T02:15:46Z -->

## Implementation report

Implementation routing started.

Worktree/branch:
- Worktree: `.worktree/simplify-ticket-identity-fields`
- Branch: `simplify-ticket-identity-fields`
- Base/routing commit: `48efcc7 ticket: route identity simplification`

Spawned sibling Coder Pod:
- `coder-simplify-ticket-identity-fields`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Routing context:
- The workspace was clean and no active implementation worktree existed before acceptance.
- This is a broad/high-risk schema/storage migration, so the handoff emphasizes audit-first implementation, complete local Ticket record migration, buildable coherent stopping points, and escalation if permanent legacy compatibility or a larger storage-provider redesign is discovered.

---

<!-- event: decision author: orchestrator at: 2026-06-09T03:03:16Z -->

## Decision

Human completion-boundary decision for this Ticket:

For `simplify-ticket-identity-fields`, stop at the merge-complete boundary.

Reason:
- The branch migrates local Ticket records from the old bucket layout to the new flat identity/state schema.
- At merge time, the repository data layout changes, while the currently running Orchestrator/Ticket tools may still be the pre-merge binary/schema.

Operational rule:
- Continue coder fix-loop and reviewer workflow normally.
- If reviewer approves and post-review checks are clean, merge the branch.
- After the merge commit and immediate validation/build as appropriate, stop and report.
- Do not close the Ticket, delete the worktree/branch, or continue queued routing until the runtime/tooling has been deliberately refreshed or the user gives explicit follow-up instructions for post-migration completion.

This Ticket intentionally overrides the broader standing instruction to auto-close/cleanup reviewer-approved work, because the migration changes the Ticket storage schema used by those completion tools.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T03:17:20Z -->

## Implementation report

Coder fix-loop completed and is ready for sibling review.

Coder Pod:
- `coder-simplify-ticket-identity-fields`
- Implementation commit: `191a875 ticket: simplify ticket identity fields`
- Fix-loop commit: `591db3f test: update ticket schema expectations`
- Worktree status before review: clean branch `simplify-ticket-identity-fields`
- Stopped after collecting output to reclaim delegated worktree scope.

Fix-loop summary:
- Updated Ticket crate tests from stale bucket/status/workflow_state/slug expectations to the new flat layout, canonical ID, and `state` schema.
- Updated close tests to distinguish `done` vs `closed` and assert `state: closed` plus `resolution.md`.
- Updated doctor/tool tests for legacy bucket detection, obsolete frontmatter field detection, invalid `state`, canonical ID lookup, `state` filters, and orchestration-plan canonical-ID references.
- No unresolved design conflict was reported.

Coder validation reported passed:
- `cargo test -p ticket` -> 59 passed, 0 failed
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor` -> `doctor: ok`
- `cargo check --workspace`
- `nix build .#yoi`

Completion boundary reminder:
- Per human instruction, if review approves and the branch merges cleanly, stop at merge-complete/validation boundary. Do not close this Ticket or clean up the worktree/branch until runtime/tooling is refreshed or explicit follow-up is given, because the merge changes the Ticket storage schema/layout.

---

<!-- event: review author: reviewer-simplify-ticket-identity-fields at: 2026-06-09T03:27:14Z status: request_changes -->

## Review: request changes

Review result: request_changes

The core direction is aligned, but current authoritative surfaces still expose old `status` / `pending` / `slug` / bucket-model behavior, and focused CLI/panel tests fail.

Blocking issues:

1. Ticket tool/backend surface still exposes `TicketStatus` / `pending`.
   - `crates/ticket/src/tool.rs` still registers `TicketStatus` as a mutating tool.
   - `TicketStatusParam::{Open, Pending}` and `TicketStatusTool` still accept/return pending-style state.
   - `crates/ticket/src/lib.rs` still has `set_status()` mapping old `TicketStatus::Pending` into planning.
   - This violates the invariant that pending bucket/status is removed and `state` is canonical.

2. Panel/TUI still has pending/status dependencies.
   - `crates/tui/src/workspace_panel.rs` `derive_ticket_state()` still treats `TicketStatus::Pending` specially for Defer.
   - `cargo test -p tui ticket_action --no-run` fails with stale references (`TicketStatus`, `workflow_state_field`, old `TicketPanelEntry` fields, pending defer action test).

3. CLI unit tests still assert old slug/status/bucket behavior.
   - `cargo test -p yoi ticket_cli` fails: 5 passed, 3 failed.
   - Failing tests still pass `--slug` and panic on `unknown create argument: --slug`.

4. `AGENTS.md` still presents old commands/layout as authoritative guidance.
   - Mentions `--slug`, `--kind`, `--label`, `--status`, `open|pending|closed`, `id-or-slug`, `.yoi/tickets/{open,pending,closed}/<id>/`, and close moving to `closed/`.

Positive evidence:
- Flat `.yoi/tickets/<ticket-id>/` layout is present.
- `cargo run -q -p yoi -- ticket doctor` passes with `doctor: ok`.
- `ticket list --state all` and exact ID show work.
- Old slug lookup fails as expected.
- `cargo test -p ticket`, `cargo fmt --check`, `git diff --check`, `cargo check --workspace`, and `nix build .#yoi` passed.

Required fix direction:
- Remove or explicitly retire `TicketStatus` / `pending` from current Ticket tools/backend mutation surface, or make retained compatibility non-current and not user/tool-visible.
- Update panel/current action model to use canonical `state` only, with no pending/defer status path.
- Update `cargo test -p yoi ticket_cli` tests to new CLI behavior: `create --title`, `list --state`, `state <id> ...`, exact ID lookup, flat path assertions.
- Update TUI/panel tests to compile and assert canonical ID/state behavior.
- Update `AGENTS.md` to the new Ticket commands/layout and exact ID guidance.

---

<!-- event: implementation_report author: coder at: 2026-06-09T04:04:33Z -->

## Implementation report

Fix loop update: removed current TicketStatus mutating tool/backend status move path and pending status handling; updated panel/TUI action model to state-only Queue/Close/Edit/Wait/Open without Defer; refreshed yoi ticket CLI tests to canonical ID, `list --state`, `state`, and flat `.yoi/tickets/<ticket-id>/` assertions; updated AGENTS.md current Ticket guidance. Validation passed locally: `cargo test -p ticket`, `cargo test -p yoi ticket_cli`, `cargo test -p tui ticket_action --no-run`, `cargo test -p tui workspace_panel`, `cargo test -p tui ticket_action`, `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, `cargo check --workspace`, and `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T04:06:54Z -->

## Implementation report

Coder fix-loop for reviewer request_changes completed and is ready for re-review.

Coder Pod:
- `coder-simplify-ticket-identity-fields`
- New fix commit: `21114fd fix: remove stale ticket status surfaces`
- Branch status after fix: clean
- Stopped after collecting output to reclaim delegated worktree scope.

Fix summary:
- Removed current `TicketStatus` mutating tool surface and related schema registration.
- Removed backend `set_status()` and stale pending status mutation path.
- Removed panel/TUI `Defer` / pending-status current action path and updated focused tests to canonical `state` behavior.
- Updated yoi CLI tests away from `--slug`, `--status`, bucket paths, and old lookup expectations toward canonical IDs, `list --state`, `state <ticket-id> ...`, and flat paths.
- Updated `AGENTS.md` to flat `.yoi/tickets/<ticket-id>/`, canonical exact ID, `state` lifecycle, and no slug/bucket/pending authority.

Coder validation reported passed:
- `cargo test -p ticket`
- `cargo test -p yoi ticket_cli`
- `cargo test -p tui ticket_action --no-run`
- `cargo test -p tui workspace_panel`
- `cargo test -p tui ticket_action`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor` -> `doctor: ok`
- `cargo check --workspace`
- `nix build .#yoi` (dirty-tree warning only; build succeeded)

Coder reports no remaining risks or unresolved removal conflicts.

---
