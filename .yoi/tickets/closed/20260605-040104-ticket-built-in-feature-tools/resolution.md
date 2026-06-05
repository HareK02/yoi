Ticket built-in feature tools are complete and merged.

Implementation:

- `afd7f04 feat: add built-in ticket tools`
- merge commit: `4486a81 merge: add ticket feature tools`

Summary:

- Added the MVP Ticket tool surface:
  - `TicketCreate`
  - `TicketList`
  - `TicketShow`
  - `TicketComment`
  - `TicketReview`
  - `TicketStatus`
  - `TicketClose`
  - `TicketDoctor`
- Kept Ticket tool behavior in `crates/ticket/src/tool.rs`.
- Kept `pod` as a thin adapter in `crates/pod/src/feature/builtin/ticket.rs`.
- Added `HostAuthority::TicketBackend { root }` to distinguish typed Ticket backend authority from generic filesystem access.
- Wired Ticket tools through the existing built-in feature contribution / ToolRegistry path.
- Resolved local backend root as `<workspace>/work-items`; unusable roots register no Ticket tools and emit a warning diagnostic.
- Did not add Ticket tools to `crates/tools`.
- Did not shell out to `tickets.sh`.
- Did not implement Intake workflow, Orchestrator routing, TUI changes, external tracker integration, MCP/plugin loading, scheduler/lease behavior, or feature-api extraction.

Review:

- External sibling reviewer approved with no blockers.
- Non-blocker follow-ups:
  - Feature install report currently records Ticket as installed even when backend root is unusable and no tools are registered; diagnostics may deserve more visible surfacing later.
  - `HostAuthority::TicketBackend { root }` currently uses the pre-canonical backend path while backend use is canonicalized; future explicit grant comparisons should normalize consistently.

Post-merge validation passed:

- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p pod feature --lib`
- `cargo test -p pod --lib`
- `cargo test -p tools --lib`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- `nix build .#yoi --no-link`

This clears the tool-surface prerequisite for `ticket-intake-workflow` and `ticket-orchestrator-routing`.
