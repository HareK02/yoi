Closed as superseded by concrete MCP implementation Tickets.

This Ticket bundled config/trust policy, stdio lifecycle, tools/list registration, tools/call execution, resources/prompts operations, result serialization, and list_changed handling into one broad implementation item. That is too coarse for the current Ticket policy: Tickets should be concrete implementation tasks.

The MCP roadmap now lives in Objective `00001KTR80WMN` (`MCP local stdio integration roadmap`). Concrete follow-up Tickets are:
- `00001KVHR3WRF` — local stdio server config and trust policy;
- `00001KVHR3WRY` — stdio JSON-RPC lifecycle client;
- `00001KVHR3WS6` — server tools registration into ToolRegistry;
- `00001KVHR3WSD` — tools/call execution through ordinary Tool path;
- `00001KVHR3WSN` — resources/prompts as explicit tool operations;
- `00001KVHR3WSW` — list_changed notification handling.

Future MCP work should use those concrete Tickets or similarly scoped follow-ups, not this broad umbrella Ticket.
