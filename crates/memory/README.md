# memory

Defines shared Subject Memory candidates, evidence, extraction progress, and Host communication.
Does not own persistence, execution authority, or a Workspace-wide Memory store.
Hosts bind operations to explicit Subjects; `subjektiv` owns the stored Memory domain.

## Main entry points

- [`subject`]: Subject-bound candidate operations and resident surface responses
- [`extract`]: candidate records and extraction progress payloads
- [`schema`]: bounded evidence and Session source references
