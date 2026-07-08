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

<!-- event: implementation_report author: hare at: 2026-07-08T12:37:56Z -->

## Implementation report

Implemented workspace-scoped Workspace/Profile settings in the delegated worktree.

Summary:
- Added scoped Workspace settings API to read/update display metadata through the workspace identity record with revision checks.
- Added scoped Profile settings API for workspace Decodal Profile registry/source discovery, create/update/delete, default selector updates, typed diagnostics, revision conflict checks, safe source-root/path validation, and Decodal/ProfileSourceArchive validation.
- Added Backend launch-options integration so project profile candidates refresh from the same settings projection; project-profile spawns carry the resolved ConfigBundle through the Runtime resource-fetch/config boundary.
- Added Settings page UI for workspace display name, profile registry/default selector, profile source creation/edit/delete, and safe diagnostics without exposing raw host paths, secrets, runtime endpoints/tokens, socket/session/store paths, archive content/digest, resource handles, or runtime internals.

Validation:
- git diff --check
- cargo test -p yoi-workspace-server
- cargo check -p yoi
- cd web/workspace && deno task check && deno task test
- yoi ticket doctor
- nix build .#yoi --no-link


---
