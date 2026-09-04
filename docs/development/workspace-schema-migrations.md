# Workspace database schema baseline

The Workspace Server owns one control-plane SQLite database. New databases are created directly from the current canonical schema; the repository does not retain an executable chain of historical Workspace schema migrations.

Domain components such as Ticket and Merge Request contribute their current tables to the same database, but they do not create a second Workspace authority.

## Compatibility boundary

The Server accepts only the current canonical schema generation. Its `__yoi_schema_migrations` ledger must contain exactly one row naming that baseline. A database with an older, newer, or multi-generation Workspace migration history is rejected at startup.

This is intentional while Yoi has only the dogfooding deployment. Schema changes may replace the baseline rather than adding permanent compatibility code. Existing dogfooding data must be migrated manually and atomically before starting the new binary.

## Updating the dogfooding database

1. Stop every Server and Runtime process that can write the affected SQLite or Runtime stores.
2. Record the current binary revision and schema generation.
3. Take a SQLite-safe backup of `server.db` and a filesystem backup of any Runtime stores whose persisted contracts change.
4. Apply the data and schema repair explicitly. Keep Workspace SQL data and Runtime filesystem data as separate authorities; changing one does not repair the other.
5. Replace historical migration-ledger rows with the single marker expected by the current baseline.
6. Validate before startup:

   ```sql
   PRAGMA foreign_key_check;
   PRAGMA integrity_check;
   ```

7. Start exactly one Server generation and verify the affected API contracts.

There is no in-place down migration and no automatic upgrade from an old baseline. Rollback means restoring both the prior binary and the complete matching database and Runtime-store backups.

## Creating a new baseline

A baseline change must include:

- canonical DDL that creates a fresh database directly at the new generation;
- current-schema verification for Workspace, Ticket, and Merge Request tables;
- tests proving a fresh database records only the canonical baseline marker;
- an explicit, separately reviewed repair procedure for the current dogfooding data;
- removal of obsolete migration functions, fixtures, commands, and documentation.

Do not put temporary legacy interpretation into normal request or projection paths. If persisted Runtime data also changes identity or shape, repair that Runtime authority explicitly instead of teaching steady-state Workspace APIs to accept both contracts indefinitely.
