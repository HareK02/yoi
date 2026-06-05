<!-- event: create author: tickets.sh at: 2026-06-05T04:01:04Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T05:37:41Z -->

## Decision

Decision: keep Ticket tool behavior mostly in the `ticket` crate and use `pod` only as a thin built-in feature adapter.

Rationale:

- `ticket` should own the Ticket domain/backend and the behavior of Ticket operations.
- `pod::feature` is still owned by the `pod` crate, so feature installation/authority wiring cannot be fully crate-contained without extracting a lower `feature-api` crate.
- Extracting `feature-api` is explicitly out of scope for this ticket.
- Putting Ticket tools in `crates/tools` would repeat the Task tool responsibility problem.

Dependency shape:

```text
ticket -> llm-worker   # Tool/ToolDefinition implementation only
pod -> ticket          # built-in feature adapter and backend-root wiring
```

Forbidden:

```text
ticket -> pod
```

Implementation direction:

- `crates/ticket` may add Ticket tool input/output types and Tool implementations.
- `crates/pod/src/feature/builtin/ticket.*` should only install/register those tools through `FeatureContribution` and resolve host/backend authority.
- Ticket tools must operate through typed `TicketBackend`/`LocalTicketBackend`, not through `tickets.sh` or generic filesystem writes.


---

<!-- event: plan author: hare at: 2026-06-05T05:37:42Z -->

## Plan

Preflight result: `implementation-ready`.

Prerequisites are complete:

- `ticket-local-files-backend` added the typed Ticket backend and `LocalTicketBackend`.
- `feature-api-authority-separation` clarified host-authority naming in `pod::feature`.

Scope for implementation:

- Add the MVP Ticket tool surface: create/list/show/comment/review/status/close/doctor.
- Keep Ticket tool behavior in `crates/ticket` and use a thin `pod` adapter for built-in feature registration and backend-root/authority wiring.
- Do not implement Intake workflow, Orchestrator routing, TUI changes, external trackers, MCP/plugin loading, scheduler/lease behavior, or `work-items/` rename.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-05T05:58:38Z status: approve -->

## Review: approve

# External review: Ticket built-in feature tools

## 1. Result

approve

## 2. Summary of implementation

The implementation adds Ticket tool behavior in `crates/ticket/src/tool.rs`, exposes the eight MVP tools (`TicketCreate`, `TicketList`, `TicketShow`, `TicketComment`, `TicketReview`, `TicketStatus`, `TicketClose`, `TicketDoctor`) over `LocalTicketBackend`, and wires them into `pod` through a thin built-in feature adapter in `crates/pod/src/feature/builtin/ticket.rs`.

The Pod side declares `HostAuthority::TicketBackend { root }`, validates/canonicalizes `<workspace>/work-items` before registering tools, and contributes the tool definitions through the existing feature contribution path. No Ticket implementation was added to `crates/tools`, and the implementation does not shell out to `tickets.sh`.

## 3. Requirement-by-requirement assessment

- Ticket crate owns tool types and `llm-worker::tool::Tool` implementations: satisfied. `crates/ticket/src/tool.rs` contains the input/output structs, schemas, tool definitions, and backend calls; `ticket` does not depend on `pod`.
- Pod adapter is thin: satisfied. `crates/pod/src/feature/builtin/ticket.rs` resolves the local backend root, declares feature metadata/authority, and registers `ticket::tool::ticket_tools(...)`.
- No Ticket tools in `crates/tools`: satisfied. Focused search found no Ticket tool code under `crates/tools`.
- Tool names/schemas: satisfied. The implemented names match the requested MVP surface, and schemas are generated from typed serde/schemars input structs.
- Tools use `LocalTicketBackend`, not `tickets.sh` or generic filesystem writes: satisfied. Tool execution calls typed backend methods directly.
- Backend root resolution: satisfied for this slice. The adapter resolves `<workspace>/work-items`, canonicalizes it, requires it to be a directory with `open/`, `pending/`, and `closed/`, and registers no tools when unusable.
- `HostAuthority::TicketBackend { root }`: satisfied. A Ticket-specific authority is used rather than generic filesystem authority, and each registered tool contribution requires that authority.
- Normal PreToolCall/tool permission path: satisfied. Tools are registered through the normal Worker/ToolRegistry path; no bypass was introduced.
- Bounded, model-readable outputs: satisfied. List/doctor/show outputs are JSON with explicit limits and truncation metadata; show truncates item/thread/resolution bodies and bounds returned event/artifact metadata.
- `tickets.sh doctor` compatibility: satisfied by review and tests present in the implementation. Create/comment/review/status/close paths write the existing work-item layout and thread event format; `TicketStatus` now appends a status event while preserving doctor compatibility.
- Existing backend behavior: no regression found in the touched backend code. The only backend behavior change in this commit is status-change thread event emission plus `updated_at` via the shared append path.
- `Cargo.lock` / `package.nix`: changes appear necessary and safe for the new `ticket -> llm-worker/schemars/serde/...` and `pod -> ticket` dependencies; `cargoHash` was updated accordingly.
- Out-of-scope work: no Intake workflow, Orchestrator routing, TUI changes, scheduler, external tracker, MCP/plugin loading, feature-api extraction, or unrelated refactor was found.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

- The feature install report records the Ticket feature as installed even when the backend root is unusable and no tools are registered. This is fail-closed for tool availability, but surfacing the diagnostic more visibly may be useful later if users expect Ticket tools and the root is missing.
- `HostAuthority::TicketBackend { root }` is declared from the pre-canonical backend path while the actual backend uses the canonicalized root. This is acceptable in the current grant-all built-in host flow, but future explicit grant/config comparisons should use a normalized representation consistently.

## 6. Validation assessed or rerun

Reran focused read-only validation:

```text
cd /home/hare/Projects/yoi/.worktree/ticket-built-in-feature-tools && git diff --check develop...HEAD && ./tickets.sh doctor
```

Result:

```text
doctor: ok
```

Also inspected:

- `git diff develop...HEAD` / changed file list
- `crates/ticket/src/lib.rs`
- `crates/ticket/src/tool.rs`
- `crates/pod/src/feature/builtin/ticket.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/feature.rs`
- relevant Cargo/package changes
- absence of Ticket tool code in `crates/tools`

I did not rerun `cargo test`, `cargo check`, or `nix build` because this external review was constrained to not modify source/worktree state; those commands normally write build artifacts.

## 7. Residual risk

The remaining risk is mainly operational: full compile/test/Nix validation was not rerun by this reviewer. Based on code inspection and the focused checks above, the implementation matches the agreed architecture and is ready to merge after the normal build/test evidence is accepted.


---

<!-- event: implementation_report author: hare at: 2026-06-05T05:58:39Z -->

## Implementation report

# Implementation report: ticket-built-in-feature-tools

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-built-in-feature-tools`
- Branch: `work/ticket-built-in-feature-tools`

## Commit

- `afd7f04 feat: add built-in ticket tools`

## Summary

Added the MVP Ticket tool surface as built-in Pod tools while preserving the agreed ownership split:

- `crates/ticket` owns Ticket tool behavior.
- `crates/pod` owns only the built-in feature adapter and backend-root/host-authority wiring.
- `crates/tools` remains unchanged and does not contain Ticket tools.

The implementation exposes Ticket operations through typed `LocalTicketBackend` calls rather than shelling out to `tickets.sh` or granting generic filesystem write access.

## Final module layout

- `crates/ticket/src/tool.rs`
  - Ticket tool input/output types.
  - JSON-schema-backed tool definitions.
  - Bounded JSON output shaping.
  - `llm_worker::Tool` implementations.
  - `ticket_tools(...)` factory.
- `crates/ticket/src/lib.rs`
  - Exports `pub mod tool`.
  - Updates `set_status` to append a `status_changed` event so status moves remain doctor-clean.
- `crates/pod/src/feature/builtin/ticket.rs`
  - Thin built-in Ticket feature adapter.
  - Resolves and validates `<workspace>/work-items`.
  - Declares `HostAuthority::TicketBackend { root }`.
  - Registers Ticket tools through `FeatureContribution`.
- `crates/pod/src/feature/builtin.rs`
  - Exports the built-in Ticket feature adapter.
- `crates/pod/src/controller.rs`
  - Installs the Ticket built-in feature alongside existing built-ins.
- `crates/pod/src/feature.rs`
  - Adds explicit `HostAuthority::TicketBackend { root }`.

## Tool surface

Implemented tools:

- `TicketCreate`
- `TicketList`
- `TicketShow`
- `TicketComment`
- `TicketReview`
- `TicketStatus`
- `TicketClose`
- `TicketDoctor`

Input schemas are generated from typed serde/schemars structs. Outputs are bounded JSON in `ToolOutput.content` with concise summaries.

Bounds include:

- `TicketList`: default/max `100` / `200`.
- `TicketShow`: recent events default/max `20` / `100`; artifacts default/max `50` / `200`; Markdown body bytes default/max `16 KiB` / `64 KiB`.
- `TicketDoctor`: diagnostics default/max `100` / `500`.

`TicketStatus` intentionally handles `open` and `pending`; `closed` requires `TicketClose` so `resolution.md` is written.

## Backend root / authority wiring

- Pod adapter resolves the local backend root as `<workspace>/work-items`.
- The root is canonicalized and validated to contain `open/`, `pending/`, and `closed/`.
- Missing/unusable roots register no Ticket tools and emit a warning diagnostic; arbitrary directories are not silently created.
- Registered tools require typed `HostAuthority::TicketBackend { root }`, not generic filesystem authority.
- Tool calls still use the normal feature contribution / ToolRegistry / PreToolCall path.

## Changed files

- `Cargo.lock`
- `crates/pod/Cargo.toml`
- `crates/pod/src/controller.rs`
- `crates/pod/src/feature.rs`
- `crates/pod/src/feature/builtin.rs`
- `crates/pod/src/feature/builtin/ticket.rs`
- `crates/ticket/Cargo.toml`
- `crates/ticket/src/lib.rs`
- `crates/ticket/src/tool.rs`
- `package.nix`

## Evidence Ticket tools are not in `crates/tools`

- No files under `crates/tools` were modified.
- Ticket tool implementation is in `crates/ticket/src/tool.rs`.
- Pod only adapts/registers the tools through the feature layer.

## Validation

Coder-reported validation passed:

- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p pod feature --lib`
- `cargo test -p pod --lib`
- `cargo test -p tools --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

Reviewer-rerun validation passed:

- `git diff --check develop...HEAD`
- `./tickets.sh doctor`

## Review status

External sibling reviewer approved with no blockers.

## Non-blocker follow-ups

- Feature install report currently records Ticket as installed even when backend root is unusable and no tools are registered. Fail-closed behavior is correct, but diagnostics may deserve more visible surfacing later.
- `HostAuthority::TicketBackend { root }` is declared from the pre-canonical backend path while the backend uses the canonicalized root. This is acceptable with the current built-in grant flow, but future explicit grant comparisons should normalize consistently.

## Ready for merge

Yes.


---
