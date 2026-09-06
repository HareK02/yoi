# Workspace schema migrations

Workspace schema authority belongs to `crates/workspace-server/src/store.rs`. The canonical
schema, ordered migrations, migration-history validation, startup upgrade path, and explicit
`yoi-server migrate` command must remain one contract.

## Retained migration chain

Released or dogfooded schema migrations are retained and composed in version order. Adding a new
schema version does not authorize deleting the preceding migration. A migration may be removed
only as an explicit baseline-retirement operation after the supported installations that depend on
it have been migrated or intentionally discarded.

The current retained Workspace chain is:

1. schema 50: `workspace schema baseline`
2. schema 51: `workspace runtime bindings`
3. schema 52: `workspace Runtime binding revision and audit`
4. schema 53: `durable Workspace deletion operations`

A database may begin at any retained baseline. Its following history rows must be the exact prefix
of the ordered migration chain from that baseline. This allows both a freshly created current
database and a database upgraded across several releases while rejecting edited, reordered, or
unknown histories.

## Runtime behavior

`SqliteWorkspaceStore::open` computes all pending retained migrations and applies them in order.
Each migration is transactional and restartable: if a later step fails, completed steps remain a
valid canonical prefix and the next run resumes from that version.

`yoi-server migrate` invokes the same store migration path without starting the Server:

```sh
# Copy the DB into memory and validate the complete pending path without changing the source.
yoi-server migrate --dry-run

# Preflight the complete path, then apply it to the source DB.
yoi-server migrate
```

Use `--database <PATH>` for a non-default Server DB. Stop `yoi-server` before applying migrations
and make an external backup before an operational upgrade.

## Development workflow

When changing the Workspace schema:

1. increment `LATEST_SCHEMA_VERSION`;
2. append one `Migration` entry with the new version, stable name, and apply function;
3. preserve all migrations at or above `OLDEST_SCHEMA_VERSION`;
4. update the canonical latest-schema creator for fresh databases;
5. add a fixture at the oldest retained version and prove migration through every retained step;
6. prove that `--dry-run` leaves the source DB unchanged;
7. keep DDL validation and cross-schema foreign-key checks in the shared store preparation path.

A deliberate baseline retirement must be a separately reviewed change. It must identify the oldest
remaining version, provide an operational migration/discard plan for older databases, update tests
and this document, and must not be inferred merely because a new migration was added.
