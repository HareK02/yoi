# ticket

Handles Ticket requirements, states, relations, audit events, and per-Workspace SQLite persistence.
Does not search Repository documents or handle Git integration.

## Main entry points

- [`TicketBackend`], [`SqliteTicketBackend`]: operation boundary and persistence
- [`TicketBackend::update_case`]: reasoned close, reopen, and closure correction
- [`ticket_content_digest`]: requirement-content comparison condition
- [`tool`]: Worker-facing operation definitions
