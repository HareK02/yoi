## Workspace Drive

Drive is Backend-owned workspace storage, not a Workdir or a host filesystem path. Feature/Profile activation is not a grant; every operation is checked by the Backend. Use the exposed Drive tools or the native WIP `/drive` entry with bounded listing and reading. Do not enumerate a Drive tree or infer authority from a visible reference.

Observe an entry before replacing or deleting it; the host/client retains the last committed request observed for that entry and checks it again when writing. On conflict, reread and reconcile rather than overwriting blindly. On `OutcomeUnknown`, retain the request ID and query request status before another mutation: the operation may have committed. Save a Workdir file into Drive only through a current scoped attachment using its logical relative source path; saving does not attach a Workdir or grant file access.

Internal SubWorkers currently have no explicit Backend Drive grant mechanism. They must not inherit Drive access through a parent's shared WorkspaceClient, even with an enabled or inherited Profile. Ask the authorized parent to perform the operation instead.
