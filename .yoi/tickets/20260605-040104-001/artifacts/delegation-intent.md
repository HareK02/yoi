# Delegation intent: Ticket built-in feature tools

## Classification

`implementation-ready` with one important architectural constraint:

- Ticket domain/backend and Ticket tool behavior should live in the `ticket` crate as much as possible.
- `pod` should contain only the thin built-in feature adapter: descriptor, host-authority/backend-root wiring, and feature contribution registration.

This avoids repeating the earlier Task split problem where stateful feature behavior lived in the generic `tools` crate while lifecycle/state ownership lived in `pod`.

## Intent

Expose typed Ticket operations as built-in Pod tools without granting arbitrary filesystem write scope over `work-items/`.

The tools must operate through the `ticket` crate's `TicketBackend` / `LocalTicketBackend`, not through `tickets.sh` shell execution and not through generic file write tools.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/ticket-built-in-feature-tools`
- branch: `work/ticket-built-in-feature-tools`

## Architecture decision

Use this dependency/ownership shape:

```text
crates/ticket
  - Ticket domain/backend
  - Ticket tool input/output types
  - Ticket tool implementations as llm-worker Tool impls
  - ticket_tools(...) factory

crates/pod
  - built-in Ticket feature descriptor
  - local backend root resolution, initially <workspace>/work-items
  - host authority declaration/grant wiring
  - FeatureContribution registration

crates/tools
  - unchanged; do not add Ticket tools here
```

Allowed dependency direction:

```text
ticket -> llm-worker   # for Tool/ToolDefinition only
ticket -> serde/schemars/etc.
pod -> ticket
```

Forbidden dependency direction:

```text
ticket -> pod
```

Do not extract a lower `feature-api` crate in this ticket. A fully crate-contained feature contribution can be revisited later if/when feature API is moved below `pod`.

## Requirements

- Add Ticket tool behavior to the `ticket` crate.
- Add a thin `pod::feature::builtin::ticket` adapter that installs those tools through the existing FeatureContribution/ToolRegistry path.
- Do not put Ticket tools in `crates/tools`.
- Do not shell out to `tickets.sh`.
- Use `LocalTicketBackend` over the configured/local backend root.
- Initial backend root may be resolved as `<workspace>/work-items` by the Pod adapter.
- Register Ticket tools only when the built-in Ticket feature is explicitly installed by the Pod host adapter.
  - It is acceptable for the current Pod controller to install it when `<workspace>/work-items` exists or when the project config path is otherwise clearly resolved.
  - If the root is missing/unusable, fail closed or do not register tools; do not create arbitrary directories silently.
- Treat Ticket operations as typed backend authority, not generic filesystem write scope.
- If adding a new `HostAuthority` variant is appropriate, prefer an explicit Ticket/backend authority over reusing generic `Filesystem` in a way that implies arbitrary FS access.
- Preserve existing PreToolCall/tool permission path: registered tools still go through normal tool-call policy.
- Keep outputs bounded and model-readable.
- Keep write paths compatible with `./tickets.sh doctor`.
- Add focused tests for both the `ticket` crate tool behavior and the Pod feature adapter.

## Tool surface

Implement the MVP tool set unless a clear blocker appears:

- `TicketCreate`
- `TicketList`
- `TicketShow`
- `TicketComment`
- `TicketReview`
- `TicketStatus`
- `TicketClose`
- `TicketDoctor`

Optional follow-up, not required for MVP:

- `TicketArtifactWrite`
- `TicketArtifactRead`
- `TicketSearch`

## Suggested tool semantics

Keep exact schemas practical, but preserve these intentions:

- `TicketList`
  - filter by status (`open`, `pending`, `closed`, `all`) and optionally label/kind/priority if easy.
  - output summaries, not full bodies by default.
- `TicketShow`
  - read one ticket by id/slug/query and return bounded item/thread/resolution/artifact metadata.
- `TicketCreate`
  - create a Ticket with title, optional slug/kind/priority/labels, and Markdown body sections.
  - do not create unresolved drafts unless the caller asks for an explicit requirements-sync/spike-style Ticket body.
- `TicketComment`
  - append a typed event role: `comment`, `plan`, `decision`, or `implementation_report`.
- `TicketReview`
  - append approve/request-changes review.
- `TicketStatus`
  - move among open/pending/closed where supported; prefer `TicketClose` for close-with-resolution.
- `TicketClose`
  - close with Markdown resolution.
- `TicketDoctor`
  - report backend doctor diagnostics in bounded form.

## Non-goals

- Implementing Intake workflow/profile.
- Implementing Orchestrator routing/scheduling.
- Renaming `work-items/`.
- Removing `tickets.sh`.
- External tracker integration.
- MCP/plugin loading.
- Scheduler/lease/queue automation.
- TUI changes.
- Moving feature API into a new crate.
- Adding Ticket tools to `crates/tools`.

## Current code map

- `crates/ticket/src/lib.rs`
  - Ticket domain/backend, `TicketBackend`, `LocalTicketBackend`, local tests.
  - Add `tools` module or submodule here for Ticket tool behavior if the file is getting large.
- `crates/ticket/Cargo.toml`
  - May need `llm-worker`, `serde`, `serde_json`, `schemars`, `async-trait`, and possibly `tokio` depending on tool implementation style.
- `crates/pod/src/feature/builtin.rs`
  - Add/export built-in Ticket module.
- `crates/pod/src/feature/builtin/ticket.rs` or `crates/pod/src/feature/builtin/ticket/mod.rs`
  - Thin adapter that contributes Ticket tools.
- `crates/pod/src/controller.rs`
  - Installs built-in features. Add Ticket feature installation only through an explicit, bounded backend-root resolution path.
- `crates/pod/src/feature.rs`
  - Host authority definitions if a Ticket/backend authority variant or tests are needed.
- `crates/pod/Cargo.toml`
  - Add dependency on `ticket`.

## Critical risks

- Putting Ticket tool implementations in `pod` would undercut the purpose of extracting the Ticket backend and make the feature harder to reuse.
- Putting Ticket tools in `tools` would repeat the Task-tools responsibility problem.
- Making Ticket tools equivalent to generic filesystem write scope would violate the authority model.
- Auto-creating `work-items/` in arbitrary workspaces would surprise users; fail closed or do not register when no backend root is configured/found.
- Returning unbounded full ticket/thread/artifact content would make tool output too large.

## Validation

Run at least:

- `cargo test -p ticket`
- focused Pod feature tests, e.g. `cargo test -p pod ticket --lib` if test names allow it
- `cargo test -p pod feature --lib`
- `cargo test -p pod --lib`
- `cargo test -p tools --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- final module layout;
- exact tool names/schemas/output style;
- how backend root/authority is wired;
- evidence Ticket tools are not in `crates/tools`;
- validation results;
- unresolved risks/follow-ups;
- whether ready for external review.
