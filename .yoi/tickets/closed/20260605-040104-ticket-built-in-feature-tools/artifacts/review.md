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
