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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T07:13:47Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVSKGDYS`.

Implementation commit:
- `31565c9b feat: persist workspace identity`

Changed areas:
- Added tracked project identity record:
  - `.yoi/workspace.toml`
  - current contents are safe project identity fields only: `workspace_id`, `created_at`, `display_name`。
- Added Workspace identity schema/loader:
  - `crates/workspace-server/src/identity.rs`
  - strict TOML parser with `workspace_id`, `created_at`, `display_name`。
  - UUIDv7 validation。
  - UTC RFC3339 `created_at` validation。
  - unknown fields rejected。
  - create-if-missing behavior。
  - invalid existing file fails closed without rewrite。
  - tests for missing creation, stable existing identity, invalid file, no path leakage。
- Integrated persisted identity into workspace server/bootstrap:
  - `crates/workspace-server/src/main.rs`
  - `crates/workspace-server/src/server.rs`
  - workspace API uses persisted `workspace_id` / display metadata。
- Updated repository ID behavior:
  - `crates/workspace-server/src/repositories.rs`
  - local repository ids derive from persisted Workspace id。
  - legacy `/api/repositories/local` remains accepted as an alias。
- Host ID derivation now receives persisted Workspace id through existing API flow。
- Added dependencies / packaging update:
  - `crates/workspace-server/Cargo.toml`
  - `Cargo.lock`
  - `package.nix` cargo hash。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && deno task check && deno task build`: passed
- `git diff --check`: passed
- `git diff --cached --check`: passed
- `cargo run -p yoi -- ticket doctor`: passed
- `nix build .#yoi --no-link`: passed

Generated artifact cleanup:
- Coder removed generated frontend artifacts:
  - `web/workspace/.svelte-kit`
  - `web/workspace/build`
  - `web/workspace/node_modules`
- Coder removed generated Rust build cache:
  - `target`

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVSKGDYS-workspace-identity` at `31565c9b`。
- Diff `4cda83b7..HEAD` touches expected Workspace identity/backend/package files and adds `.yoi/workspace.toml`。
- `git diff --check 4cda83b7..HEAD`: passed。

Next action:
- Route to read-only Reviewer Pod for acceptance review。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-23T07:14:37Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVSKGDYS-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVSKGDYS-workspace-identity`
- Branch: `impl/00001KVSKGDYS-workspace-identity`
- Baseline: `4cda83b7`
- Implementation commit: `31565c9b feat: persist workspace identity`
- Diff: `4cda83b7..31565c9b`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Safety and tracked-project-record suitability of `.yoi/workspace.toml`。
- Stable persisted id usage in Workspace API / repository / host ids。
- Missing/existing/invalid file behavior。
- No local path/secret leakage。
- Package/Nix updates。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-orchestrator at: 2026-06-23T07:24:58Z status: request_changes -->

## Review: request changes

Reviewer Pod `yoi-reviewer-00001KVSKGDYS-r1` requested changes.

Reviewed target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVSKGDYS-workspace-identity`
- Branch: `impl/00001KVSKGDYS-workspace-identity`
- Implementation: `31565c9b feat: persist workspace identity`
- Diff: `4cda83b7..31565c9b`

Blocking issue:
1. Missing-file create path can return/use an identity that is not the persisted `.yoi/workspace.toml` identity under a local bootstrap race。

Evidence:
- `crates/workspace-server/src/identity.rs` generates a new `WorkspaceIdentity`, calls `identity.write_new(path)?`, and then returns the generated identity。
- In `write_new`, if another process creates `workspace.toml` after the initial missing-file path, the code validates the existing file but discards the parsed identity:

```rust
if path.exists() {
    let _ = fs::remove_file(&tmp);
    let raw = fs::read_to_string(path)?;
    return Self::parse_str(&raw, path).map(|_| ());
}
```

- The caller still returns the newly generated identity, so a server instance can upsert SQLite / serve Workspace API / repository / host ids using an id that is not the tracked file and will not survive restart。
- Related TOCTOU risk: `path.exists()` followed by `fs::rename(tmp, path)` can overwrite a concurrently created file on Unix, and fixed temp path `workspace.toml.tmp` can be clobbered by concurrent creators。

Required fix direction:
- Ensure the returned `WorkspaceIdentity` is always the identity actually persisted or read from `.yoi/workspace.toml`。
- Use a no-overwrite / create-new style finalization or equivalent safe retry/read behavior for local bootstrap races。
- Add focused test coverage for the race/return-authority behavior if feasible。

Non-blocking reviewer findings:
- `.yoi/workspace.toml` contents look safe to track。
- Unknown fields are intentionally denied and documented/tested。
- `ServerConfig::local_dev` no longer derives `local:{display}` internally。
- Workspace API/store/repository/host plumbing appears correctly based on persisted identity once loaded。
- Existing `local:*` rows are not destructively migrated。

Reviewer validation all passed before request_changes:
- `cargo fmt --check`
- `git diff --check 4cda83b7..HEAD`
- `cargo test -p yoi-workspace-server` (`15 passed`)
- `cargo check -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task build`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link`

Reviewer notes generated ignored artifacts from validation remained in worktree:
- `target/`
- `web/workspace/node_modules/`
- `web/workspace/.svelte-kit/`
- `web/workspace/build/`

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-23T07:25:27Z -->

## Comment

Rerouted request-changes feedback to Coder Pod `yoi-coder-00001KVSKGDYS`.

Requested fix:
- `WorkspaceIdentity::load_or_init` must return the identity actually persisted/read from `.yoi/workspace.toml`。
- Missing-file creation must handle local bootstrap races without returning an unpersisted generated identity。
- Avoid or safely handle TOCTOU / fixed temp-file clobbering around `workspace.toml.tmp`。
- Add focused deterministic test coverage if feasible。

Validation requested after fix:
- `cargo fmt --check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi-workspace-server`
- `cd web/workspace && deno task check && deno task build`
- `git diff --check`
- `git diff --cached --check`
- `cargo run -p yoi -- ticket doctor`
- `nix build .#yoi --no-link`

Coder was also asked to remove ignored validation artifacts before reporting。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T07:30:46Z -->

## Implementation report

Coder fix report received from `yoi-coder-00001KVSKGDYS` after request_changes。

New commit:
- `49c9e190 fix: return persisted workspace identity`

Fix summary:
- Replaced temp-file + `rename` finalization in `crates/workspace-server/src/identity.rs`。
- New creation path uses `OpenOptions::create_new(true)` directly on `.yoi/workspace.toml`。
- If creation succeeds, generated identity is written and returned。
- If creation fails because the file already exists from a concurrent bootstrap race, the existing file is read/parsed and that persisted identity is returned。
- Removed fixed `workspace.toml.tmp` path and the `path.exists()` + `rename` TOCTOU overwrite risk。

Test coverage:
- Added deterministic unit test `create_new_race_returns_existing_persisted_identity`。
- Test verifies an already-persisted identity wins over a generated candidate, returned identity matches the file, and the file is not rewritten。
- Existing tests for missing creation, stable existing identity, invalid fail-closed behavior, no path leakage, and unknown field rejection remain。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p yoi-workspace-server`: passed
- `cargo check -p yoi-workspace-server`: passed
- `cd web/workspace && deno task check && deno task build`: passed
- `git diff --check`: passed
- `git diff --cached --check`: passed
- `cargo run -p yoi -- ticket doctor`: passed
- `nix build .#yoi --no-link`: passed

Cleanup reported:
- Removed ignored validation artifacts:
  - `target/`
  - `web/workspace/node_modules/`
  - `web/workspace/.svelte-kit/`
  - `web/workspace/build/`

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVSKGDYS-workspace-identity` at `49c9e190`。
- Diff `4cda83b7..HEAD` still touches expected Workspace identity/backend/package files。
- Latest fix commit changes only `crates/workspace-server/src/identity.rs`。
- `git diff --check 4cda83b7..HEAD`: passed。
- Spot-check of `identity.rs` confirmed `write_new_or_read_existing` returns parsed persisted identity on `AlreadyExists` and no longer uses fixed temp file / rename finalization。

Next action:
- Route back to Reviewer Pod for re-review of request_changes fix。

---
