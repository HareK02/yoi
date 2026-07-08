<!-- event: create author: "yoi ticket" at: 2026-07-08T08:34:01Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-08T10:40:06Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-08T10:40:06Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-08T10:44:13Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-08T10:45:48Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Declared dependency `00001KWZ5KERY` is now closed, so the original Decodal/ProfileSourceArchive prerequisite is satisfied。
- However `00001KX0G06VA` is currently `inprogress` on branch `work/00001KX0G06VA-resource-fetch-api`。
- This Ticket’s Workspace/Profile editing API and pages need to build on the same profile/resource/workspace-server launch surfaces that `00001KX0G06VA` is actively changing（ProfileSourceArchive resource handling, workspace-server settings/API/hosts/server, Browser-facing profile launch configuration）。
- Starting this Ticket now would create a high-conflict parallel branch and risks designing Profile editing against the wrong resource-fetch/public API boundary。
- Therefore this routing pass leaves the Ticket queued and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX0DSMPT)`: depends_on `00001KWZ5KERY`; target is closed。
- `TicketOrchestrationPlanQuery(00001KX0DSMPT)`: prior record 0 件だったため、今回 `after 00001KX0G06VA` と waiting-capacity note を記録。
- `TicketList`: queued はこの Ticket 1件、inprogress は `00001KX0G06VA` 1件。
- Orchestrator worktree git status: clean on `orchestration`。

Next action:
- `00001KX0G06VA` の implementation/review/merge outcome を待つ。
- resource-fetch API shape が merge されたら、この Ticket を再 routing し、updated orchestration branch から start する。

Escalate if:
- 人間が `00001KX0G06VA` を中断してこの Ticket を先に実装する、または両者を同一 combined worktree で統合実装する方針に切り替えたい場合。

---

<!-- event: decision author: orchestrator at: 2026-07-08T11:56:03Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Previous waiting reason is resolved: `00001KX0G06VA` is now merged/validated/closed, so the ProfileSourceArchive resource-fetch API shape this Ticket depends on is stable enough to build against。
- Declared dependency `00001KWZ5KERY` is also closed, so Decodal Profile source model / ProfileSourceArchive boundary is available。
- Ticket body gives concrete Workspace/Profile settings API, read/write source model, UI routes, diagnostics, redaction invariants, and validation criteria。
- typed relation blocker is resolved (`depends_on 00001KWZ5KERY` target closed); no active inprogress Ticket remains。
- The user requested sequential consumption once no blocker remains, and the Dashboard queue authorization is already present。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX0DSMPT)`: outgoing `depends_on 00001KWZ5KERY`; target is closed。
- `TicketOrchestrationPlanQuery(00001KX0DSMPT)`: prior `after 00001KX0G06VA` / waiting note; `00001KX0G06VA` is now closed with merge commit `01f94b75` and close commit `a645ee5b`。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: this Ticket 1件。inprogress は 0 件。

IntentPacket:

Intent:
- Workspace-scoped Browser API and pages for Workspace metadata and Profile registry/source editing, using Backend as the authority and the Decodal/ProfileSourceArchive model already merged。
- Frontend should edit Profiles through safe Backend APIs, not filesystem paths, and Worker launch profile candidates should reflect updated Backend discovery/validation state。

Binding decisions / invariants:
- Workspace/Profile editing APIs live under `/api/w/<workspace-id>/settings...` or consistent scoped Workspace API paths。
- Browser-facing API/UI must not expose host absolute paths, secret values, Runtime endpoints/tokens, socket/session paths, runtime-local store paths, archive content/digest, or raw resource handles。
- Profile source references use safe ids/artifact ids/display paths rather than raw absolute paths。
- Writes must validate syntax/schema/selector uniqueness/path safety before commit and return typed diagnostics。
- Decodal/ProfileSourceArchive source model from `00001KWZ5KERY` and resource-fetch boundary from `00001KX0G06VA` are authority inputs; do not reintroduce Runtime filesystem discovery。
- Runtime direct sync is out of scope; Backend discovery/launch options invalidation is in scope。

Requirements / acceptance criteria:
- Workspace metadata read/update API works for display name and safe identity/source summaries。
- Profile registry/source read/update APIs support create/update/delete with revision/etag-style optimistic check。
- Settings UI has Workspace overview and Profile list/source editor routes under `/w/<workspace-id>/settings...`。
- Profile updates re-run Backend discovery/validation and update Worker launch profile candidates。
- Duplicate selector, invalid registry schema, invalid Decodal syntax, path/symlink escape, artifact too large, and concurrent update conflict return typed diagnostics。
- Browser WorkingDirectory / Worker launch profile candidates and settings Profile list share the same Backend discovery result。

Implementation latitude:
- Endpoint naming, module boundaries, revision token shape, source/artifact id representation, UI component split, and editor implementation can follow existing workspace-server / Svelte style。
- v0 plain textarea/editor is acceptable; no heavyweight editor required。
- Imported source write support can be limited to bounded module root or deferred with typed diagnostics if not needed for core v0。

Escalate if:
- Secret value editor, multi-user auth/RBAC, Runtime direct sync, Plugin/MCP/sandbox artifact sync, Ticket/Objective/Memory editor integration, or a new Profile source semantic model beyond Decodal/ProfileSourceArchive is required。

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map:
- Workspace/profile settings backend: `crates/workspace-server/src/server.rs`, `hosts.rs`, `config.rs`, `resource_broker.rs`, Decodal/archive helpers。
- Profile archive/config: `crates/worker-runtime/src/profile_archive.rs`, `config_bundle.rs` for source model reference。
- Frontend settings: `web/workspace/src/routes/w/[workspaceId]/settings/**`, `web/workspace/src/lib/workspace-settings/**`, worker launch profile candidate helpers。

Critical risks / reviewer focus:
- No raw paths/secrets/runtime internals or resource handles leak to Browser。
- Profile editing updates the same Backend discovery/launch options used by Worker launch。
- Optimistic concurrency and path/symlink escape checks fail closed。
- Decodal validation is real and not only superficial string checks。
- Existing Runtime resource fetch / ProfileSourceArchive authority boundaries remain intact。

---

<!-- event: state_changed author: orchestrator at: 2026-07-08T11:56:36Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after previous waiting reason was resolved.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX0DSMPT)`: depends_on `00001KWZ5KERY`; target is closed。
- `TicketOrchestrationPlanQuery(00001KX0DSMPT)`: prior `after 00001KX0G06VA` / waiting note を確認。`00001KX0G06VA` is now closed / merged。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。

Acceptance basis:
- Previous blocker/waiting reason is resolved。
- concrete missing decision / information は残っていない。
- Ticket scope is Workspace/Profile settings API and pages using already-merged Decodal/ProfileSourceArchive and resource-fetch boundaries。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-08T11:58:14Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KX0DSMPT-workspace-profile-settings`
  - Branch: `work/00001KX0DSMPT-workspace-profile-settings`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KX0DSMPT-profile-settings` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-08T12:03:23Z -->

## Implementation report

Implementation routing recovery note:

- Initial Coder Pod `yoi-coder-00001KX0DSMPT-profile-settings` failed its first turn with provider `context_length_exceeded` before producing implementation output。
- Ticket remains `inprogress`; worktree/branch already exist and are clean at creation baseline。
- Recovery plan: avoid `StopPod` and retry with a fresh minimal Coder task that reads the Ticket records from the worktree instead of embedding the full IntentPacket in the prompt. If scope registry prevents a fresh Pod, fallback is to use the existing Pod with a much smaller instruction or ask for human cleanup/authority。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-08T12:40:07Z -->

## Implementation report

Implementation progress report:

- Recovery Coder Pod `yoi-coder2-00001KX0DSMPT-profile-settings` completed implementation and committed `3eefd333 feat: add workspace profile settings` on branch `work/00001KX0DSMPT-workspace-profile-settings`.
- Worktree status was clean after commit。
- Changed files include:
  - `crates/workspace-server/src/profile_settings.rs` (new)
  - `crates/workspace-server/src/{server.rs,hosts.rs,companion.rs,lib.rs}`
  - `web/workspace/src/lib/workspace-settings/{profile-api.ts,profile-types.ts,model.ts}`
  - `web/workspace/src/lib/workspace-api/http.ts`
  - `web/workspace/src/routes/w/[workspaceId]/settings/+page.svelte`
  - child implementation also committed `.yoi/tickets/00001KX0DSMPT/{item.md,thread.md}` changes; Orchestrator will preserve authoritative Ticket thread/item records during integration if conflicts occur。
- Implementation summary:
  - Added scoped Workspace/Profile settings API for workspace metadata and profile registry/source read/write。
  - Added Backend profile settings model with safe source ids/display paths, revision checks, Decodal validation, selector uniqueness checks, path/symlink escape protection, and typed diagnostics。
  - Profile updates rebuild Backend profile discovery/launch options so Worker launch profile candidates and settings profile list use the same discovery result。
  - Extended settings UI with Workspace overview and Profiles list/editor surface using scoped APIs and safe summaries。
  - Browser-facing responses avoid raw host paths, runtime internals, archive digests/content, and resource handles。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p yoi-workspace-server`: pass（80 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（17 tests）
  - `yoi ticket doctor`: ok
  - `nix build .#yoi --no-link`: pass
- External review will be requested via a read-only sibling Reviewer Pod. `StopPod` は使わない。

---

<!-- event: review author: reviewer at: 2026-07-08T12:49:09Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blockers:

1. Profile mutation validation failures are not returned as typed diagnostics。
   - scoped settings handlers call `profile_settings::*` with `?` directly (`server.rs:881-945`).
   - `ApiError::from(Error)` initializes `diagnostics: Vec::new()` (`server.rs:3355-3360`).
   - profile errors such as `profile_selector_duplicate`, `profile_source_syntax_invalid`, `profile_source_path_escape`, `profile_registry_revision_conflict` fall through to `BAD_GATEWAY` (`server.rs:3403-3414`) with an empty diagnostics array (`server.rs:3420-3423`).
   - Recorded requirement says duplicate selector, invalid registry/schema/Decodal, path/symlink escape, artifact too large, and concurrency conflicts must return typed diagnostics.

2. Worker launch profile candidates can include invalid project profiles。
   - `load_profile_settings` attaches Decodal/source validation errors from `validate_project_profiles` only to top-level `diagnostics` (`profile_settings.rs:327`, `663-707`).
   - each project `WorkspaceProfileSummary` only gets `source_summary` diagnostics (`profile_settings.rs:304-323`).
   - `worker_profile_candidates_for_root` includes project profiles when `profile.diagnostics.is_empty()` (`server.rs:2989-2992`).
   - `is_profile_candidate` accepts any discovered project profile id (`profile_settings.rs:629-635`).
   - Result: registry entry with invalid Decodal source can still appear in launch options and be accepted as a candidate until spawn-time archive resolution fails.
   - This violates the intent that profile updates re-run Backend discovery/validation and Worker launch candidates share the same effective validated discovery result.

3. Registry/default updates can commit references that are not actually Backend-published/valid。
   - `validate_default_profile` accepts any string beginning with `project:` without checking that the project selector exists in submitted registry or discovery result (`profile_settings.rs:946-955`).
   - `update_profile_registry` only validates source content if the expected source file already exists (`profile_settings.rs:463-468`) before writing the registry (`profile_settings.rs:470`).
   - This permits API writes that persist nonexistent project defaults or missing project source references, contrary to the invariant that writes validate schema/selector/source references before commit and defaults/candidates are Backend-published.

Validation performed:
- Read Ticket `00001KX0DSMPT` item/thread, IntentPacket, binding invariants, acceptance criteria, implementation report, and reviewer focus。
- Read-only inspection in implementation worktree `/home/hare/Projects/yoi/.worktree/00001KX0DSMPT-workspace-profile-settings`。
- Inspected implementation commit scope and relevant backend/frontend files。
- Relied on Orchestrator-provided validation results; did not modify files or run root/original workspace operations。

Non-blocking follow-ups:
- Negative-test coverage is thinner than reviewer focus: duplicate selector, invalid registry schema, symlink escape, artifact size, redaction, concurrent update API response shape, and launch-candidate invalidation need stronger coverage.
- UI implements profile list/editor on single `/settings` page with anchors/forms rather than separate `/settings/profiles/...` pages. This may be acceptable under implementation latitude, but confirm route granularity if it matters.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-08T13:07:39Z -->

## Implementation report

Review-fix implementation report:

- Recovery Coder Pod completed requested changes and committed follow-up `0c2ca1ea fix: harden profile settings validation` on branch `work/00001KX0DSMPT-workspace-profile-settings`.
- Worktree status was clean after commit。
- Fix summary:
  - Profile settings validation errors now map to typed Browser diagnostics with non-empty `diagnostics` arrays and appropriate HTTP statuses（revision conflict 409、invalid/duplicate/path/schema/source/profile errors 400、unknown source/selector 404）。
  - Project profile discovery attaches source/Decodal validation failures to the relevant `WorkspaceProfileSummary`。
  - Invalid project profiles are excluded from Worker launch candidates and rejected by candidate validation。
  - Registry/default writes validate default selector existence, selector/source consistency, missing source files, path/symlink escape, source size, Decodal/ProfileSourceArchive validity, and invalid registry schema before commit。
  - Added/strengthened tests for duplicate selector, invalid Decodal/schema, stale revision conflict, too-large source, symlink escape redaction, invalid launch candidate rejection, and missing/default reference rejection。
- Orchestrator reran validation in the implementation worktree:
  - `git diff --check`: pass
  - `cargo test -p yoi-workspace-server`: pass（91 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
  - `cd web/workspace && deno task test`: pass（17 tests）
  - `yoi ticket doctor`: ok
  - `nix build .#yoi --no-link`: pass
- Requesting follow-up external review against `3eefd333..0c2ca1ea` and full combined implementation。

---
