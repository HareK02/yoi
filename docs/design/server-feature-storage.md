# Server-managed Feature storage

`yoi-workspace-server::feature_storage` provides the SQLite lifecycle boundary for trusted Server-side Features. It is not a Worker feature API and it is not a model-visible SQL tool. Subject-scoped Memory is persisted in the Workspace's `subjektiv` Feature database through this boundary; legacy single-document Workspace Memory remains in the control-plane database and is not owned, imported, or reset by `FeatureStorage`.

## Ownership boundary

The Server owns:

- the physical layout under `feature-storage/<workspace-id>/features/<feature-id>.sqlite`;
- validated Workspace and Feature identifiers (no caller-provided database path);
- connection reuse, `foreign_keys`, WAL, `busy_timeout`, and write serialization;
- migration timing/history, shutdown admission, online backup/restore, and Workspace deletion fencing.

A Feature owns:

- its tables, migration bodies, queries, and domain transactions;
- validation and typed repository/API methods exposed to its Server handler;
- authorization before the repository is called.

A Feature database is an API/ownership isolation boundary inside the trusted Server process. It does not sandbox arbitrary native code and does not replace Workspace authorization. A `FeatureDatabase`, SQLite path, or general SQL endpoint must never be sent to a Worker/Runtime or added to a model tool contract.

## Registration and repository example

Registration is trusted startup code. Merely constructing or registering storage is lazy and creates no directory or database; the first `open` applies migrations.

```rust
use rusqlite::{Transaction, params};
use yoi_workspace_server::{
    FeatureDatabase, FeatureMigration, FeatureRegistration, FeatureStorageError,
    WorkspaceApi,
};

type StorageResult<T> =
    std::result::Result<T, FeatureStorageError>;

fn create_subject_memory(tx: &Transaction<'_>) -> StorageResult<()> {
    tx.execute_batch(
        "CREATE TABLE subjects (
             subject_id TEXT PRIMARY KEY,
             revision INTEGER NOT NULL
         );
         CREATE TABLE memories (
             subject_id TEXT NOT NULL,
             memory_id TEXT NOT NULL,
             body TEXT NOT NULL,
             PRIMARY KEY (subject_id, memory_id),
             FOREIGN KEY (subject_id) REFERENCES subjects(subject_id)
         );
         CREATE TABLE interventions (
             intervention_id TEXT PRIMARY KEY,
             subject_id TEXT NOT NULL,
             detail TEXT NOT NULL,
             FOREIGN KEY (subject_id) REFERENCES subjects(subject_id)
         );"
    )?;
    Ok(())
}

static MIGRATIONS: &[FeatureMigration] = &[
    FeatureMigration::new(1, "subject memory baseline", create_subject_memory),
];

const REGISTRATION: FeatureRegistration =
    FeatureRegistration::new("subjektiv", MIGRATIONS);

struct SubjektivRepository {
    database: FeatureDatabase,
}

impl SubjektivRepository {
    fn apply_candidate(
        &self,
        subject_id: &str,
        memory_id: &str,
        body: &str,
        intervention_id: &str,
    ) -> StorageResult<()> {
        // Candidate application, memory revision, and intervention recording are
        // one atomic transaction in the subjektiv Feature database.
        self.database.transaction(|tx| {
            tx.execute(
                "INSERT INTO memories (subject_id, memory_id, body)
                 VALUES (?1, ?2, ?3)",
                params![subject_id, memory_id, body],
            )?;
            tx.execute(
                "UPDATE subjects SET revision = revision + 1
                 WHERE subject_id = ?1",
                [subject_id],
            )?;
            tx.execute(
                "INSERT INTO interventions (intervention_id, subject_id, detail)
                 VALUES (?1, ?2, 'candidate applied')",
                params![intervention_id, subject_id],
            )?;
            Ok(())
        })
    }
}

fn initialize_subjektiv_repository(api: &WorkspaceApi) -> StorageResult<SubjektivRepository> {
    let registered = api.feature_storage().register(REGISTRATION)?;
    let database = api.feature_storage().open(&registered)?;
    Ok(SubjektivRepository { database })
}
```

A production Feature should register once while constructing its Server service and retain the returned `RegisteredFeature`/typed repository. Duplicate registration is rejected. `FeatureDatabase` methods are synchronous because `rusqlite` is synchronous; async HTTP/background handlers must run nontrivial repository work on a blocking executor rather than blocking a Tokio core thread.

## Migration and transaction contract

- Versions are contiguous from 1. Migration schema changes and the corresponding history rows are committed in one `BEGIN IMMEDIATE` transaction.
- Concurrent opens in one manager are single-flight. Independent Server managers/processes serialize migration writers through SQLite and reread history after acquiring the transaction.
- A migration callback error rolls back the complete migration batch. A later open retries it; failure is never recorded as success.
- Stored migration names are verified. A history mismatch is rejected, and a schema newer than the registered plan is never overwritten or downgraded.
- `FeatureDatabase::transaction` covers multiple tables in one Feature database. No atomic transaction is promised with the Server control-plane database or another Feature database.
- A domain rejection can return `FeatureStorageError::operation(...)`; the transaction then rolls back.

## Lifecycle and operations

### Disable and restart

Not registering/opening a disabled Feature creates nothing. Disabling a previously used Feature does not delete its database. On restart, registering and opening the same Feature/Workspace reuses the persisted file and verifies/applies its migrations.

### Backup and restore

`WorkspaceFeatureStorage::backup` uses SQLite's online backup API for each discovered Feature database, so a live WAL is not ignored or copied as a bare database file. Each image passes `integrity_check` and `foreign_key_check`, and a manifest binds the snapshot to its Workspace and Feature identifiers. The snapshot directory is published with an atomic rename only after all images succeed.

Each Feature image is internally consistent. There is deliberately no cross-database snapshot instant or distributed transaction between Feature databases or the Server DB. A caller that needs a higher-level coordinated backup must first stop relevant Feature writers. In particular, the [subjektiv product cutover](../development/subjektiv-product-cutover.md) stops legacy and subjektiv activity before pairing a control-plane backup with the subjektiv Feature backup; the explicit legacy reset never deletes or recreates the Feature database.

`restore` rejects the wrong Workspace manifest, corrupt/foreign-key-invalid images, an existing destination, or any live connection. It builds validated databases in a temporary directory and atomically publishes the Workspace storage directory. Restore is an operator/Server lifecycle operation, not a model API.

### Workspace deletion

Workspace deletion calls `WorkspaceFeatureStorage::delete` after Worker/Workdir cleanup and before finalizing removal from the Server DB. It fences new opens, waits for connection-locked operations, closes handles, and removes only the validated Workspace subtree. The admission fence remains until the cached Workspace API is discarded, preventing a background task from recreating storage between filesystem deletion and control-plane finalization. Retrying deletion is idempotent.

### Shutdown

After HTTP graceful drain, Server serve paths explicitly call `shutdown`. Shutdown rejects new registration/open, waits for in-progress connection operations to release their locks, closes all managed connections, and makes retained handles fail with `DatabaseClosed`. Operations must not treat a shutdown error as success.

## Failure handling

Identifier, registration, migration-history, unsupported-schema, maintenance, shutdown, snapshot, I/O, and SQLite failures are typed as `FeatureStorageError`. Server handlers should map them to a bounded domain/API error without exposing paths, SQL, or unrestricted database access. Migration/restore/delete failures must stop the corresponding lifecycle transition; they are not warnings to ignore.
