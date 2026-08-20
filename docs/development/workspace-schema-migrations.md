# Workspace database schema migration runbook

The Workspace Server owns one control-plane SQLite database. Schema changes are applied by the Server at startup; domain components such as Ticket and Merge Request contribute tables to that same database, but they do not create a second Workspace authority.

## Before deployment

1. Stop writes and shut down every Server process using the database. Do not run two Server generations against one database during migration.
2. Record the current binary revision and database schema version.
3. Take a byte-for-byte backup of the database and its WAL/SHM state using a SQLite-safe backup procedure.
4. Run the read-only plan with the new binary:

   ```sh
   yoi-server migrate --dry-run --database <server.db>
   ```

   The plan runs against an in-memory copy. It reports the current and target schema versions, migration names, Worker identity mappings, and repairs without mutating the source database. Workspace-resource preflight failures name the relation and bounded offending row identities; repair those rows through the owning domain authority before retrying.

## Applying

Start exactly one instance of the new Server binary against the database. Startup applies migration 39 in one SQLite transaction after the Ticket and Merge Request component schemas are available. The migration:

- rebuilds Ticket, Objective, assignment, Artifact, and human-key tables with Workspace-scoped composite identity;
- adds composite foreign keys for repository, Ticket, Objective, Worker, relation-target, and current-assignment references;
- validates new historical assignment/event references with SQLite triggers while allowing those audit rows to survive later Ticket or Worker retention deletion; startup treats a parent missing from every Workspace as retained history but still rejects an id that resolves only in another Workspace; reservation operation ids remain intentionally unconstrained until their resources exist;
- checks the rebuilt schema with `PRAGMA foreign_key_check` before recording the schema version; and
- restores `PRAGMA foreign_keys = ON` whether the transaction commits or rolls back.

After startup, verify:

```sql
SELECT MAX(version) FROM __yoi_schema_migrations;
PRAGMA foreign_key_check;
PRAGMA integrity_check;
```

The expected migration version is `39`, `foreign_key_check` returns no rows, and `integrity_check` returns `ok`.

## Failure and rollback

There is no in-place down migration. A failed migration transaction leaves the prior schema version and data intact. Keep the Server stopped, preserve the failure diagnostics, and either repair the preflight data with the prior generation or restore the complete pre-migration backup before retrying.

Never run an older binary after a newer schema version has committed. Startup fences this case and refuses to serve when the database schema version is newer than the binary supports. Rollback therefore means restoring both the prior binary and its matching pre-migration database backup; it does not mean pointing the old binary at the upgraded database.
