# feature-storage

Neutral Host-owned SQLite lifecycle manager for trusted Features. Owns placement,
WAL/connection settings, transaction serialization, migrations, snapshots,
restore/delete fencing, and shutdown. Features receive a managed
`FeatureDatabase` after registration. Worker/model input never selects a database
path or receives SQL access.

Use `FeatureStorage::new(trusted_root)`, `register(FeatureRegistration)`, and
`scope(host_issued_scope_id).open(&registration)`. `ScopedFeatureStorage` and
`FeatureDatabase::scope_id()` are neutral storage isolation APIs: a local Host's
persisted random scope does not create a Workspace record or imply Backend
membership. Existing placement `<root>/<scope>/features/<feature>.sqlite` and
snapshot `workspace=` keys remain unchanged for physical compatibility.

`WorkspaceFeatureStorage`, `workspace()`, `workspace_id()`, and
`for_server_database()` are Backend compatibility aliases/helpers. They do not
introduce a Server dependency. The Host is responsible for state-root privacy,
symlink/path validation, and external execution leases before constructing this
trusted manager.

`configure_connection(&rusqlite::Connection)` is reusable by other Host stores
(such as JobStore). It sets the five-second busy timeout, foreign keys ON, WAL,
and synchronous FULL before migrations/transactions. The Feature manager uses
this same helper; WAL lock-contention retries stay within the busy timeout.

Dependencies: `rusqlite` (existing workspace backup support), `thiserror`, `uuid`;
tests use `tempfile`.
