<!-- event: create author: tickets.sh at: 2026-05-31T01:00:05Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-31T01:01:09Z -->

## Decision

Initial decision note from user discussion:

- The plugin surface should mainly expose Hooks and Tools.
- MCP is related, but should be treated as one protocol-bound backend/bridge rather than the entire plugin model.
- Planned plugin mechanisms:
  - MCP for protocol-constrained external capability providers;
  - TOML/config-only hooks for simple behavior that does not need arbitrary code;
  - WASM for programmable Hooks/Tools with explicit host capabilities.
- General scripting languages were considered, but the initial direction is WASM because it offers a clearer sandbox/capability boundary.


---
