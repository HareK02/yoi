# ticket

Handles Ticket requirements, states, relations, audit events, and per-Workspace SQLite persistence.
Does not search Repository documents or handle Git integration.

## Main entry points

- [`TicketBackend`], [`SqliteTicketBackend`]: operation boundary and persistence
- [`TicketBackend::update_case`]: reasoned close, reopen, and closure correction
- [`ticket_content_digest`]: requirement-content comparison condition
- [`tool`]: Worker-facing operation definitions

## Posts and creation metadata

The initial body is a real `Comment` in `typed_ticket_events`, with the same author,
creation time, persisted `event_ref`, and edit metadata as later comments.
`Ticket.document.body` is a derived convenience, not another storage authority.
`TicketMeta.created_at` and optional `created_by` belong to the Ticket itself;
unknown legacy authors remain unknown.

`TicketBackend::edit_post_with_snapshots` edits a comment by its persisted identity
and exact expected body, returning the before/after Ticket snapshots atomically for
host audit and initial-post rescope handling. The host must supply the authenticated
editor, rather than trusting an author from Web input. Original post author and time
are retained; `edited_at` and `edited_by` record the edit. System events/judgments
are not editable posts. `edit_item_with_snapshots` still supports title/target and
initial-body authoring, without generating a discussion message.

Transport operation `TicketBackendOperation::EditPost { id, edit }` returns
`TicketBackendOperationResult::TicketSnapshots { previous, current }` by calling
that atomic snapshot method. The native `TicketEditPost` tool accepts `ticket`,
`event_ref`, `expected_body`, and `body`; it does not accept an author. Its standalone
actor default must be replaced with the authenticated caller by Workspace hosts.
Generic Show always returns `initial_post`, even when the recent-event window omits
it, using the same post identity/authorship/edit metadata projection as discussion
entries. Truncated bodies are marked and must not be used as exact edit observations.

Component migration 12 appends to the retained 10/11 migration history. It reserves
SQL event index `-1` for the initial comment and leaves all prior event indices,
references, attributes, and evidence unchanged. Create/item_edit records are archived
in place (`archived = 1`) outside `Ticket.events`, not deleted. New creates insert
only the real initial comment; new item edits do not append events. Existing posts
retain their published identity: a stored `event_id`, otherwise the old zero-based
`sequence:<ordinal>` computed before inserting or hiding rows. Duplicate identities
abort migration rather than being silently changed. `event_ref` is persisted and
must not subsequently be recomputed from display position; existing attributes are
not overwritten. Initial edit metadata comes only from the last known body-edit
record (`changes` includes `body`), never a later title-only edit. Missing or empty
creator identities remain explicitly nullable, not inferred from later editors.
The retained base SQL and frozen fixture schemas remain unchanged; fresh creation
composes the ordered post migration.
