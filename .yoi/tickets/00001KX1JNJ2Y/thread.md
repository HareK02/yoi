<!-- event: create author: "yoi ticket" at: 2026-07-08T19:18:25Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-09T07:18:33Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-09T07:18:33Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-09T07:56:07Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-09T07:57:36Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Declared dependency `00001KWZ5KERY` is closed, so Decodal ProfileSourceArchive / archive verify / Runtime FSなし resolution の基礎は available。
- Related follow-up `00001KX0DSMPT` Workspace/Profile settings API/pages is also closed, so settings/profile editing surface exists and this Ticket can refine it to ProfileSourceTree virtual filesystem + Decodal editor。
- Ticket body gives concrete model/API/UI/editor/dependency/validation requirements and clear non-goals。
- typed relation blocker is resolved; OrchestrationPlan record は 0 件。inprogress Ticket も 0 件。
- queued notification is human authorized routing/start。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX1JNJ2Y)`: outgoing `depends_on 00001KWZ5KERY`; target is closed。
- `TicketOrchestrationPlanQuery(00001KX1JNJ2Y)`: 0 件。
- `TicketList`: queued はこの Ticket 1件、inprogress は 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- Relevant prior work: Decodal/ProfileSourceArchive closed, Workspace/Profile settings closed。

IntentPacket:

Intent:
- Add Backend-owned `ProfileSourceTree` virtual filesystem for Decodal profile sources, connect ProfileSourceArchive import closure building to that tree, strengthen Runtime archive import validation, and add Workspace settings Profile source tree editor using `decodal-codemirror@0.1.2` / CodeMirror 6。

Binding decisions / invariants:
- Browser/editor works with virtual relative paths and safe source tree/file ids, never host absolute paths or raw filesystem authority。
- Backend validation is authority; Browser editor diagnostics are only assistance。
- Runtime reads only ProfileSourceArchive manifest / virtual paths, not Workspace filesystem。
- `ProfileSourceArchive` is a snapshot of source tree revision + selected root + import closure。
- Browser-facing API must not expose host absolute path, symlink target path, Runtime endpoint/socket/session path, secret values, WorkingDirectory root path, raw resource handles, archive digest/content, or backend-private paths。
- `decodal-codemirror@0.1.2` / CodeMirror 6 dependency is in scope; formatter and Browser-side Decodal materialization as authority are out of scope。

Requirements / acceptance criteria:
- Implement typed `ProfileSourceTree` model with tree id, revision/generation, virtual root metadata, files, provenance, diagnostics, and safe summaries。
- Implement virtual import resolver for relative imports and adopted namespace imports, with normalization and typed diagnostics for root escape, absolute path, URL/unsupported scheme, extension/content-type, depth/count/bytes limits。
- Build `ProfileSourceArchive` from selected root virtual path and source tree import closure, including virtual path / source key / digest / size / content type / import map in manifest。
- Runtime `ArchiveSourceLoader` validates manifest-contained virtual paths only and rejects manifest-outside import, absolute path, `..`, unsupported namespace。
- Add scoped Profile settings file-tree API: list trees, list files, read/write/create/delete file, validate tree/file; rename/move optional。
- Add Settings UI profile source tree list and file tree editor routes/pages using Decodal CodeMirror extension and Backend save/create/delete/validate APIs。
- Focused backend/runtime/web tests cover path normalization, import closure, archive manifest validation, Runtime import resolution, Browser redaction, file tree API, and editor route smoke。

Implementation latitude:
- Exact endpoint names, source tree storage format, namespace enum details, UI route/component split, and editor model can follow existing workspace-server/Svelte style。
- v0 may keep source tree storage minimal and project/workspace-backed, provided Browser contract remains virtual-path based and Backend validation remains authority。

Escalate if:
- Browser-facing raw host path authority, Runtime workspace filesystem discovery, Browser-side Decodal materialization as authority, formatter integration, plugin/MCP/sandbox artifact sync, or Profile source RDB-normalized editor becomes required。

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Current code map:
- Backend profile settings/source model: `crates/workspace-server/src/profile_settings.rs`, `server.rs`, `hosts.rs`。
- Decodal/Profile archive: `crates/worker-runtime/src/profile_archive.rs`, `config_bundle.rs`。
- Frontend settings/profile editor: `web/workspace/src/routes/w/[workspaceId]/settings/**`, `web/workspace/src/lib/workspace-settings/**`。

Critical risks / reviewer focus:
- virtual paths and tar paths must not allow absolute path / `..` / symlink/path escape。
- Browser must never receive host absolute path, secret, runtime endpoint, archive digest/content, or raw handle。
- Runtime SourceLoader must be archive-manifest/import-map only, no FS/backend fallback。
- source tree validation/write must be revision-aware and fail closed with typed diagnostics。
- new editor dependency and Nix/lock updates must be correct。

---

<!-- event: state_changed author: orchestrator at: 2026-07-09T07:57:59Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX1JNJ2Y)`: depends_on `00001KWZ5KERY`; target is closed。
- `TicketOrchestrationPlanQuery(00001KX1JNJ2Y)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。inprogress は 0 件。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket scope is ProfileSourceTree virtual filesystem + Decodal editor follow-up on top of closed Decodal/ProfileSourceArchive and Workspace/Profile settings work。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: hare at: 2026-07-09T08:22:18Z -->

## Implementation report

Implemented ProfileSourceTree virtual source-tree support for project Decodal profiles.

Summary:
- Added Runtime archive manifest validation for virtual source metadata, duplicate/undeclared tar entries, scoped import-map keys, namespace/path safety, and current-source relative import resolution without filesystem reads.
- Added Backend-owned project `ProfileSourceTree` API over virtual `profiles/...` paths, revision-aware read/write/delete file operations, path validation, tree scanning, import closure/archive building from tree contents, and project-profile config bundle generation from archives.
- Extended workspace server routes and browser API/types for scoped profile source tree access without exposing host absolute paths or resource/archive internals.
- Added a Profiles settings Decodal CodeMirror editor backed by virtual paths/revision tokens only.
- Added worker-runtime tests for scoped virtual imports and import-map mismatch rejection.

Validation:
- `git diff --check`
- `cargo test -p yoi-workspace-server`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`


---
