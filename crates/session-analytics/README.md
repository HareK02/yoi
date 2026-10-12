# session-analytics

`session-analytics` reads Session JSONL and aggregates observations of tool use and
context changes. It neither runs Workers nor restores history, and reports omit
message bodies and raw tool arguments/results.

## Design entry points

- [`analyze_session`]: reading and aggregation tolerant of storage-format differences
- [`SessionReport`]: observation scope and limits on interpretation
