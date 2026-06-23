<!-- event: create author: "yoi ticket" at: 2026-06-23T06:43:28Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-23T06:44:19Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-23T06:44:19Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-23T06:47:18Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T06:49:54Z -->

## Decision

Routing decision: `implementation_ready_parallel`

Reason:
- Ticket body has concrete `workspace.toml` semantics, backend/API consistency requirements, migration-safe behavior, tests, and validation criteria。
- No relations / blockers / orchestration plan records exist。
- Active Dashboard no-auto-selection work is TUI-only and separate from Workspace backend identity persistence, so parallel implementation is acceptable。
- Orchestrator worktree is clean on `orchestration` at `13e76d35`; target worktree / branch is not present。
- Current Workspace backend has ad-hoc/stable-ish IDs; Ticket asks for tracked local identity at `.yoi/workspace.toml`。

IntentPacket:

Intent:
- Persist local Workspace identity in tracked `.yoi/workspace.toml` and use it as the stable local Workspace id across backend APIs, repository IDs, host ID derivation, and frontend display where applicable。

Binding decisions / invariants:
- `.yoi/workspace.toml` is tracked project record, not local runtime/secret file。
- It should contain only safe project identity fields, e.g. `workspace_id`, `display_name`, `created_at` or equivalent; no absolute paths, user names, socket paths, data-dir paths, tokens, or runtime secrets。
- Existing checkouts without the file must remain usable with safe auto-create or fallback behavior。
- Workspace id should not change on process restart, repo path move, or sibling worktree checkout when the file is present。
- Avoid changing Ticket/Objectives canonical authority。
- Do not confuse this with Profile/manifest/override runtime config。
- Handle invalid/corrupt `workspace.toml` fail-closed with clear diagnostic; do not silently generate a different id over a bad tracked file unless explicitly safe。

Requirements / acceptance criteria:
- Define `.yoi/workspace.toml` schema and parser/loader.
- Add create-if-missing behavior for local workspace server/bootstrap path, or a documented CLI/tool path if auto-create is unsuitable.
- Use persisted workspace id in Workspace API responses and any Repository/Host ids derived from workspace id.
- Ensure generated file is safe to commit and has no local absolute paths/secrets.
- Existing tests updated to deterministic temp workspace identity behavior。
- Add tests for missing file creation/fallback, existing stable id, invalid file error, and no local path leakage。
- Validation includes workspace-server tests, Deno check/build if frontend output changes, git diff check, Ticket doctor, and Nix build if package/source behavior changes。

Implementation latitude:
- Put parser in `workspace-server` crate if currently only the web backend needs it, or a small shared crate if needed; avoid broad architecture churn。
- Workspace id can reuse project-record id allocator if suitable, or a stable slug/uuid/base32 type if already used。
- If auto-writing tracked file during server startup is risky, implement explicit ensure function used by tests/bootstrap and document behavior; but Ticket prefers tracked persistence。
- Frontend can just display the id returned by existing `/api/workspace` if backend response changes。

Escalate if:
- Creating tracked `.yoi/workspace.toml` from a server process violates current project-record write boundaries。
- Workspace id generation requires global registry/coordination beyond local checkout。
- Existing code strongly assumes workspace id is derived from path and changing it would break multiple APIs unexpectedly。
- Nix/source filtering excludes `.yoi/workspace.toml` unexpectedly and package behavior needs product decision。

Validation plan:
- `cargo fmt --check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task build` if frontend-visible changes occur。
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link` if package/source filtering or tracked file behavior changed。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T06:50:08Z from: queued to: inprogress reason: human_authorized_unblocked_workspace_identity_persistence field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete local workspace identity requirements and no recorded blockers, so Orchestrator accepts implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:51:44Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVSKGDYS-workspace-identity`
- Created branch:
  - `impl/00001KVSKGDYS-workspace-identity`
- Base commit:
  - `4cda83b7 ticket: accept workspace identity and selection work`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVSKGDYS`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVSKGDYS-workspace-identity`

Parallelization note:
- `00001KVSKJ0EA` is active separately and targets TUI Dashboard selection semantics. This Ticket should stay limited to Workspace backend identity persistence and safe project record behavior。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---
