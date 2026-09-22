# Repository-local `.yoi` authority removal

This release removes the remaining production readers, writers, discovery helpers, fallbacks, and import paths that treated a repository-local `.yoi` tree as product authority.

Removed surfaces include:

- Ticket filesystem config loading, `LocalTicketBackend`, and local-to-SQLite import helpers;
- Worker Ticket feature constructors backed by local paths;
- Memory repository layout/discovery, local resident/query/staging/audit/usage executors, and `yoi memory lint`;
- the Runtime helper that derived its store from `<workspace>/.yoi/runtime-store`.

Workspace identity, metadata, Ticket, Objective, Memory, and launch configuration remain owned by the Workspace Server control-plane database and typed Workspace APIs. Client routing remains in XDG client configuration. Runtime-owned Workdirs, session logs, artifacts, Server data directories, repository/Git files, explicit Plugin package files, and the legacy home-directory secret-store boundary are unchanged.

There is no automatic migration or compatibility fallback. Before upgrading from a version that stored active Ticket or Memory data in a repository `.yoi` tree, explicitly export data with the old version and import it through a supported Workspace-backed surface. Stale or malformed repository/ancestor `.yoi` trees are ignored by normal operation and may be archived or removed after verification.
