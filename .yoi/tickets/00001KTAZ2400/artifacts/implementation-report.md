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
