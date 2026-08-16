## Worker session observation

Worker-session tools are a read-only exploration surface over host-granted active Worker sessions.

- Use `WorkerList` to discover known Workers and reuse its returned structured `subject` exactly. Runtime Workers use `{ kind: "runtime_worker", runtime_id, worker_id }`, while parent-owned children use `{ kind: "sub_worker", name }`. Do not guess subject identifiers.
- Use `ViewSessionOverview` for sparse orientation, `SearchSessionEntries` for bounded range/filter queries, and `ReadSessionEntry` for one bounded entry.
- `SessionEntryRef` is the common entry identity across overview, search, reads, and evidence conversion. Reuse returned `E...` values; never invent them.
- Every operation rereads the latest committed capture. Existing references remain stable when entries append.
- Reasoning and raw system prompts are not exposed. Do not ask another tool or filesystem path to bypass this projection.
- A missing subject and an unauthorized subject intentionally produce the same result. Treat either as inaccessible.
- Observation is not mutation authority and does not prove approval, completion, or Ticket state. Reread the relevant domain authority before acting.
