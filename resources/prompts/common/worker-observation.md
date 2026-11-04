## Worker session observation

Worker-session tools are a read-only exploration surface over host-granted active Worker sessions.

- Use `WorkerList` to discover known Workers and reuse its returned structured `subject` exactly. Runtime Workers use `{ kind: "runtime_worker", runtime_id, worker_id }`, while parent-owned children use `{ kind: "sub_worker", name }`. Do not guess subject identifiers.
- Use `ViewSessionOverview` for sparse orientation, `SearchSessionEntries` for bounded range/filter queries, and `ReadSessionEntry` for one bounded entry.
- `SessionEntryRef` is the common entry identity across overview, search, reads, and evidence conversion. Reuse returned `E...` values; never invent them.
- For routine progress, read new results, failures, and blockers with their stable evidence references rather than reopening and transcribing all earlier logs. Before adoption or completion, inspect the concrete evidence relevant to that decision; an inaccessible reference remains unverified. Keep handling updates focused on current status, next action, and material result references.
- Every operation rereads the latest committed capture. Existing references remain stable when entries append.
- A missing subject and an unauthorized subject intentionally produce the same result. Treat either as inaccessible.
- Observation is not mutation authority and does not prove approval, completion, or Ticket state. Reread the relevant domain authority before acting.
