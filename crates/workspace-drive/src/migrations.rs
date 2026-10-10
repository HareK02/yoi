//! Frozen schema upgrades. Old column/JSON names occur only to preserve existing data.
use crate::{Mutation, NodeId};
use feature_storage::FeatureMigration;
use rusqlite::{OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub(super) static MIGRATIONS: &[FeatureMigration] = &[
    FeatureMigration::new(1, "drive_tree_and_receipts", create_original_schema),
    FeatureMigration::new(
        2,
        "bind_nodes_to_committed_requests",
        bind_committed_requests,
    ),
    FeatureMigration::new(3, "name_legacy_replay_counter", name_legacy_replay_counter),
];
fn create_original_schema(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch("\
        CREATE TABLE drive_state (singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            workspace_id TEXT NOT NULL UNIQUE, deleting INTEGER NOT NULL DEFAULT 0 CHECK(deleting IN (0,1)));
        CREATE TABLE drive_nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            workspace_id TEXT NOT NULL REFERENCES drive_state(workspace_id),
            parent_id INTEGER,
            name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) <= 255),
            kind TEXT NOT NULL CHECK(kind IN ('file','directory')),
            revision INTEGER NOT NULL CHECK(revision > 0),
            blob_key TEXT, size INTEGER NOT NULL CHECK(size >= 0 AND size <= 16777216),
            content_type TEXT, updated_by TEXT NOT NULL, updated_at_ms INTEGER NOT NULL,
            UNIQUE(workspace_id,id),
            FOREIGN KEY(workspace_id,parent_id) REFERENCES drive_nodes(workspace_id,id),
            CHECK((kind='directory' AND blob_key IS NULL AND size=0 AND content_type IS NULL)
                OR (kind='file' AND blob_key IS NOT NULL AND content_type IS NOT NULL)));
        CREATE UNIQUE INDEX drive_sibling_name ON drive_nodes(workspace_id,ifnull(parent_id,0),name);
        CREATE INDEX drive_children ON drive_nodes(parent_id,id);
        CREATE INDEX drive_blob_reference ON drive_nodes(blob_key) WHERE blob_key IS NOT NULL;
        CREATE UNIQUE INDEX drive_single_root ON drive_nodes(workspace_id) WHERE parent_id IS NULL;
        CREATE TABLE drive_receipts (request_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL,
            result_json TEXT NOT NULL, committed_at_ms INTEGER NOT NULL);
    ")?;
    Ok(())
}

fn bind_committed_requests(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    // Materialize the original receipt correspondence once. In particular, a
    // deletion receipt contains the pre-delete node, so it refers to the earlier
    // create/update request, not to the deletion request itself. Equal timestamps
    // do not matter. Missing/ambiguous evidence fails constraints and rolls back.
    // Keep only proven predecessor correspondence for legacy digest verification.
    // No request payload is stored: fingerprints remain untouched. A missing
    // predecessor leaves replay unavailable, but the migrated result recoverable
    // through request_status. This evidence never authorizes a new mutation.
    tx.execute_batch("\
        CREATE TEMP TABLE drive_observed_requests (
            node_id INTEGER NOT NULL, previous_counter INTEGER NOT NULL,
            request_id TEXT NOT NULL, matches INTEGER NOT NULL CHECK(matches=1),
            PRIMARY KEY(node_id,previous_counter));
        INSERT INTO drive_observed_requests
            SELECT CAST(json_extract(result_json, '$.node.id') AS INTEGER),
                CAST(json_extract(result_json, '$.node.revision') AS INTEGER),
                min(request_id), count(*) FROM drive_receipts
            WHERE json_extract(result_json, '$.deleted') = 0 GROUP BY 1,2;
        CREATE TABLE drive_legacy_replays (
            request_id TEXT PRIMARY KEY REFERENCES drive_receipts(request_id) ON DELETE CASCADE,
            expected_mutation_id TEXT NOT NULL, legacy_expected_counter TEXT NOT NULL);
        INSERT INTO drive_legacy_replays
            SELECT receipt.request_id, observed.request_id, CAST(observed.previous_counter AS TEXT)
            FROM drive_receipts AS receipt JOIN drive_observed_requests AS observed
                ON observed.node_id = CAST(json_extract(receipt.result_json, '$.node.id') AS INTEGER)
                AND observed.previous_counter = CAST(json_extract(receipt.result_json, '$.node.revision') AS INTEGER)
                    - CASE WHEN json_extract(receipt.result_json, '$.deleted') = 1 THEN 0 ELSE 1 END;
        ALTER TABLE drive_nodes ADD COLUMN last_mutation_id TEXT NOT NULL DEFAULT '';
        UPDATE drive_nodes SET last_mutation_id = (
            SELECT request_id FROM drive_observed_requests
            WHERE node_id = drive_nodes.id AND previous_counter = drive_nodes.revision
        ) WHERE parent_id IS NOT NULL;
        UPDATE drive_receipts SET result_json = (
            SELECT json_remove(json_set(result_json, '$.node.last_mutation_id', observed.request_id), '$.node.revision')
            FROM drive_observed_requests AS observed
            WHERE observed.node_id = CAST(json_extract(result_json, '$.node.id') AS INTEGER)
                AND observed.previous_counter = CAST(json_extract(result_json, '$.node.revision') AS INTEGER)
        );
        ALTER TABLE drive_nodes DROP COLUMN revision;
        DROP TABLE drive_observed_requests;
    ")?;
    Ok(())
}

fn name_legacy_replay_counter(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    // Already-deployed v2 databases used this column name. New v2 applications
    // create the concrete name directly; both paths retain the same replay-only
    // decimal strings and leave the original request digests untouched.
    let old_column: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('drive_legacy_replays') WHERE name='expected_revision')",
        [],
        |row| row.get(0),
    )?;
    if old_column {
        tx.execute_batch(
            "ALTER TABLE drive_legacy_replays RENAME COLUMN expected_revision TO legacy_expected_counter;",
        )?;
    }
    Ok(())
}

// Frozen pre-cutover typed payload encoding, including field order and decimal
// strings. Never reconstruct a fingerprint from the result: compare the caller's
// complete actor/payload against the original digest. Historical counters below
// are replay-only evidence and must never participate in live node CAS.
#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum LegacyMutation<'a> {
    Update {
        id: NodeId,
        #[serde(rename = "expected_revision")]
        legacy_expected_counter: &'a str,
        content_type: &'a str,
        bytes: &'a [u8],
    },
    Relocate {
        id: NodeId,
        #[serde(rename = "expected_revision")]
        legacy_expected_counter: &'a str,
        parent: NodeId,
        name: &'a str,
    },
    Delete {
        id: NodeId,
        #[serde(rename = "expected_revision")]
        legacy_expected_counter: &'a str,
    },
}

pub(super) fn matches_legacy_replay(
    tx: &Transaction<'_>,
    request_id: &str,
    actor: &str,
    mutation: &Mutation,
    fingerprint: &str,
) -> crate::Result<bool> {
    let evidence = tx
        .query_row(
            "SELECT expected_mutation_id,legacy_expected_counter FROM drive_legacy_replays WHERE request_id=?1",
            [request_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((expected_request, counter)) = evidence else {
        // Missing prior receipt: do not guess. request_status still recovers the
        // committed result, while mutate rejects this identity without execution.
        return Ok(false);
    };
    let (expected, legacy) = match mutation {
        Mutation::Update {
            id,
            expected_mutation_id,
            content_type,
            bytes,
        } => (
            expected_mutation_id,
            LegacyMutation::Update {
                id: *id,
                legacy_expected_counter: &counter,
                content_type,
                bytes,
            },
        ),
        Mutation::Relocate {
            id,
            expected_mutation_id,
            parent,
            name,
        } => (
            expected_mutation_id,
            LegacyMutation::Relocate {
                id: *id,
                legacy_expected_counter: &counter,
                parent: *parent,
                name,
            },
        ),
        Mutation::Delete {
            id,
            expected_mutation_id,
        } => (
            expected_mutation_id,
            LegacyMutation::Delete {
                id: *id,
                legacy_expected_counter: &counter,
            },
        ),
        Mutation::CreateFolder { .. } | Mutation::CreateFile { .. } => return Ok(false),
    };
    if expected != &expected_request {
        return Ok(false);
    }
    let candidate: String = Sha256::digest(serde_json::to_vec(&(actor, legacy))?)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(candidate == fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Drive, Mutation, NodeId, RequestStatus};
    use feature_storage::{FeatureRegistration, FeatureStorage};
    use rusqlite::params;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::path::Path;

    fn seed_old(path: &Path, copies_of_current_receipt: usize) {
        let storage = FeatureStorage::new(path).workspace("ws").unwrap();
        let registration = storage
            .register(FeatureRegistration::new(
                "workspace-drive",
                &MIGRATIONS[..1],
            ))
            .unwrap();
        let database = storage.open(&registration).unwrap();
        let key = "00000000-0000-0000-0000-000000000004";
        crate::blob::BlobStore::open(&path.parent().unwrap().join("blobs"))
            .unwrap()
            .put(key, b"persisted-file")
            .unwrap();
        database.try_transaction::<_, feature_storage::FeatureStorageError>(|tx| {
            tx.execute_batch("INSERT INTO drive_state(singleton,workspace_id) VALUES(1,'ws');
                INSERT INTO drive_nodes(id,workspace_id,parent_id,name,kind,revision,size,updated_by,updated_at_ms)
                VALUES(1,'ws',NULL,'','directory',1,0,'server',42),
                      (2,'ws',1,'after','directory',2,0,'actor',42),
                      (3,'ws',1,'deleted','directory',1,0,'actor',42);
                DELETE FROM drive_nodes WHERE id=3;")?;
            let receipt = |request: &str, id: &str, name: &str, number: i64, deleted: bool| -> feature_storage::Result<()> {
                // Literal frozen typed JSON keeps this fixture independent of the
                // production legacy serializer (field order is part of the digest).
                let payload = if deleted {
                    format!(r#"["actor",{{"operation":"delete","id":"{id}","expected_revision":"{number}"}}]"#)
                } else if number > 1 {
                    format!(r#"["actor",{{"operation":"relocate","id":"{id}","expected_revision":"{}","parent":"1","name":"{name}"}}]"#, number - 1)
                } else {
                    format!(r#"["actor",{{"operation":"create_folder","parent":"1","name":"{name}"}}]"#)
                };
                let fingerprint = digest(payload.as_bytes());
                let result = json!({"node":{"id":id,"workspace_id":"ws","parent_id":"1","name":name,
                    "kind":"directory","revision":number.to_string(),"size":0,"content_type":null,
                    "updated_by":"actor","updated_at_ms":42},"deleted":deleted});
                tx.execute("INSERT INTO drive_receipts VALUES(?1,?2,?3,42)", params![request,fingerprint,result.to_string()])?;
                Ok(())
            };
            receipt("create", "2", "before", 1, false)?;
            for index in 0..copies_of_current_receipt {
                receipt(&format!("move-{index}"), "2", "after", 2, false)?;
            }
            receipt("create-deleted", "3", "deleted", 1, false)?;
            receipt("delete", "3", "deleted", 1, true)?;
            tx.execute("INSERT INTO drive_nodes(id,workspace_id,parent_id,name,kind,revision,blob_key,size,content_type,updated_by,updated_at_ms)
                VALUES(4,'ws',1,'file','file',1,?1,14,'text/plain','actor',42)", [key])?;
            let file_result = json!({"node":{"id":"4","workspace_id":"ws","parent_id":"1","name":"file",
                "kind":"file","revision":"1","size":14,"content_type":"text/plain",
                "updated_by":"actor","updated_at_ms":42},"deleted":false});
            let create_file = Mutation::CreateFile { parent: NodeId(1), name: "file".into(), content_type: "text/plain".into(), bytes: b"persisted-file".to_vec() };
            let fingerprint = digest(&serde_json::to_vec(&("actor", create_file)).unwrap());
            tx.execute("INSERT INTO drive_receipts VALUES('file-create',?1,?2,42)", params![fingerprint,file_result.to_string()])?;
            Ok(())
        }).unwrap();
    }

    #[test]
    fn upgrade_uses_receipts_not_timestamps_and_preserves_replay_and_deleted_results() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_old(&path, 1);
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_eq!(
            drive.database.schema_version().unwrap(),
            MIGRATIONS.last().unwrap().version()
        );
        assert_eq!(drive.root().unwrap().last_mutation_id, "");
        let file = drive.metadata(NodeId(4)).unwrap();
        assert_eq!(file.last_mutation_id, "file-create");
        assert_eq!(
            drive
                .read(file.id, &file.last_mutation_id, 0, 100)
                .unwrap()
                .bytes,
            b"persisted-file"
        );
        let current = drive.metadata(NodeId(2)).unwrap();
        assert_eq!(current.name, "after");
        assert_eq!(current.last_mutation_id, "move-0");
        // Original create fingerprint is unchanged, and replay never overwrites the live node.
        let replay = drive
            .mutate(
                "create",
                "actor",
                Mutation::CreateFolder {
                    parent: NodeId(1),
                    name: "before".into(),
                },
            )
            .unwrap();
        assert_eq!(replay.node.name, "before");
        assert_eq!(replay.node.last_mutation_id, "create");
        assert_eq!(drive.metadata(NodeId(2)).unwrap(), current);
        let RequestStatus::Committed { result } = drive.request_status("delete").unwrap() else {
            panic!("lost deletion receipt");
        };
        assert!(result.deleted);
        assert_eq!(result.node.last_mutation_id, "create-deleted");
        assert_eq!(result.node.id, NodeId(3));
        assert!(matches!(
            drive.metadata(NodeId(3)),
            Err(crate::Error::NotFound)
        ));
        let next = drive
            .mutate(
                "new",
                "actor",
                Mutation::CreateFolder {
                    parent: NodeId(1),
                    name: "new".into(),
                },
            )
            .unwrap();
        assert!(
            next.node.id > NodeId(3),
            "deleted IDs must not be reused after migration"
        );
        drive
            .database
            .try_with_connection::<_, feature_storage::FeatureStorageError>(|c| {
                let count: i64 = c.query_row(
                    "SELECT count(*) FROM pragma_table_info('drive_nodes') WHERE name='revision'",
                    [],
                    |r| r.get(0),
                )?;
                assert_eq!(count, 0, "old counter must not remain in the live schema");
                Ok(())
            })
            .unwrap();
    }

    fn digest(payload: &[u8]) -> String {
        Sha256::digest(payload)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn seed_update(path: &Path, missing_predecessors: bool) {
        seed_old(path, 1);
        let storage = FeatureStorage::new(path).workspace("ws").unwrap();
        let old = storage
            .register(FeatureRegistration::new(
                "workspace-drive",
                &MIGRATIONS[..1],
            ))
            .unwrap();
        let database = storage.open(&old).unwrap();
        database.try_transaction::<_, feature_storage::FeatureStorageError>(|tx| {
            let result = json!({"node":{"id":"4","workspace_id":"ws","parent_id":"1","name":"file",
                "kind":"file","revision":"2","size":14,"content_type":"text/plain",
                "updated_by":"actor","updated_at_ms":42},"deleted":false});
            let payload = br#"["actor",{"operation":"update","id":"4","expected_revision":"1","content_type":"text/plain","bytes":[112,101,114,115,105,115,116,101,100,45,102,105,108,101]}]"#;
            tx.execute("INSERT INTO drive_receipts VALUES('file-update',?1,?2,42)", params![digest(payload), result.to_string()])?;
            tx.execute("UPDATE drive_nodes SET revision=2 WHERE id=4", [])?;
            if missing_predecessors {
                tx.execute("DELETE FROM drive_receipts WHERE request_id IN ('file-create','create')", [])?;
            }
            Ok(())
        }).unwrap();
    }

    fn changed_replays() -> Vec<(&'static str, Mutation)> {
        vec![
            (
                "file-update",
                Mutation::Update {
                    id: NodeId(4),
                    expected_mutation_id: "file-create".into(),
                    content_type: "text/plain".into(),
                    bytes: b"persisted-file".to_vec(),
                },
            ),
            (
                "move-0",
                Mutation::Relocate {
                    id: NodeId(2),
                    expected_mutation_id: "create".into(),
                    parent: NodeId(1),
                    name: "after".into(),
                },
            ),
            (
                "delete",
                Mutation::Delete {
                    id: NodeId(3),
                    expected_mutation_id: "create-deleted".into(),
                },
            ),
        ]
    }

    fn fingerprints(database: &feature_storage::FeatureDatabase) -> Vec<(String, String)> {
        database
            .try_with_connection::<_, feature_storage::FeatureStorageError>(|c| {
                let mut stmt = c.prepare(
                    "SELECT request_id,fingerprint FROM drive_receipts ORDER BY request_id",
                )?;
                Ok(stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap()
    }

    #[test]
    fn fresh_schema_uses_only_the_concrete_replay_counter_column() {
        let temp = tempfile::tempdir().unwrap();
        let storage = FeatureStorage::new(temp.path().join("metadata"))
            .workspace("ws")
            .unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_current_replay_schema(&drive.database);
    }

    fn assert_current_replay_schema(database: &feature_storage::FeatureDatabase) {
        assert_eq!(
            database.schema_version().unwrap(),
            MIGRATIONS.last().unwrap().version()
        );
        database
            .try_with_connection::<_, feature_storage::FeatureStorageError>(|c| {
                let mut columns = c.prepare(
                    "SELECT name FROM pragma_table_info('drive_legacy_replays') ORDER BY cid",
                )?;
                let names = columns
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                assert_eq!(
                    names,
                    [
                        "request_id",
                        "expected_mutation_id",
                        "legacy_expected_counter"
                    ]
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn v2_replay_column_upgrade_preserves_evidence_digests_and_replay_after_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_update(&path, false);
        let original = {
            let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
            let registration = storage
                .register(FeatureRegistration::new(
                    "workspace-drive",
                    &MIGRATIONS[..2],
                ))
                .unwrap();
            let database = storage.open(&registration).unwrap();
            // Actual deployed v2 schema: the old name is a frozen DB boundary,
            // not the DDL used when applying v2 to a newly created database.
            database
                .transaction(|tx| {
                    tx.execute_batch(
                        "ALTER TABLE drive_legacy_replays RENAME COLUMN legacy_expected_counter TO expected_revision;",
                    )?;
                    Ok(())
                })
                .unwrap();
            fingerprints(&database)
        };
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_current_replay_schema(&drive.database);
        assert_eq!(fingerprints(&drive.database), original);
        drive
            .database
            .try_with_connection::<_, feature_storage::FeatureStorageError>(|c| {
                let mut rows = c.prepare(
                    "SELECT request_id,expected_mutation_id,legacy_expected_counter FROM drive_legacy_replays ORDER BY request_id",
                )?;
                let evidence = rows
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                assert_eq!(
                    evidence,
                    vec![
                        ("delete".into(), "create-deleted".into(), "1".into()),
                        ("file-update".into(), "file-create".into(), "1".into()),
                        ("move-0".into(), "create".into(), "1".into()),
                    ]
                );
                Ok(())
            })
            .unwrap();
        let nodes = drive.list(NodeId(1), None, 100).unwrap().nodes;
        for (request, mutation) in changed_replays() {
            let RequestStatus::Committed { result } = drive.request_status(request).unwrap() else {
                panic!("lost {request}")
            };
            assert_eq!(drive.mutate(request, "actor", mutation).unwrap(), result);
        }
        assert_eq!(drive.list(NodeId(1), None, 100).unwrap().nodes, nodes);
        assert_eq!(fingerprints(&drive.database), original);
        drop(drive);
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_current_replay_schema(&drive.database);
        assert_eq!(fingerprints(&drive.database), original);
        assert_eq!(drive.list(NodeId(1), None, 100).unwrap().nodes, nodes);
        for (request, mutation) in changed_replays() {
            assert!(drive.mutate(request, "actor", mutation).is_ok());
        }
    }

    #[test]
    fn upgrade_preserves_original_digests_and_never_uses_old_counters_for_new_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_update(&path, false);
        let original = {
            let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
            let old = storage
                .register(FeatureRegistration::new(
                    "workspace-drive",
                    &MIGRATIONS[..1],
                ))
                .unwrap();
            fingerprints(&storage.open(&old).unwrap())
        };
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_eq!(fingerprints(&drive.database), original);
        assert!(matches!(
            drive.mutate(
                "new-update",
                "actor",
                Mutation::Update {
                    id: NodeId(4),
                    expected_mutation_id: "2".into(),
                    content_type: "text/plain".into(),
                    bytes: b"changed".to_vec(),
                }
            ),
            Err(crate::Error::Conflict)
        ));
        assert_eq!(
            drive.request_status("new-update").unwrap(),
            RequestStatus::Uncommitted
        );
        assert_eq!(
            drive.metadata(NodeId(4)).unwrap().last_mutation_id,
            "file-update"
        );
        assert_eq!(fingerprints(&drive.database), original);
    }

    #[test]
    fn migrated_update_relocate_delete_replay_original_results_without_reexecution() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_update(&path, false);
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_eq!(
            drive.database.schema_version().unwrap(),
            MIGRATIONS.last().unwrap().version()
        );
        // Historical replay must not overwrite a newer committed mutation.
        for (id, expected) in [(NodeId(2), "move-0"), (NodeId(4), "file-update")] {
            drive
                .mutate(
                    &format!("new-{}", id.get()),
                    "actor",
                    Mutation::Relocate {
                        id,
                        expected_mutation_id: expected.into(),
                        parent: NodeId(1),
                        name: format!("new-{}", id.get()),
                    },
                )
                .unwrap();
        }
        let before = drive.list(NodeId(1), None, 100).unwrap().nodes;
        for (request, mutation) in changed_replays() {
            let RequestStatus::Committed { result } = drive.request_status(request).unwrap() else {
                panic!("lost {request}")
            };
            assert_eq!(
                drive.mutate(request, "actor", mutation).unwrap(),
                result,
                "{request}"
            );
            assert_eq!(drive.list(NodeId(1), None, 100).unwrap().nodes, before);
        }
        // The unchanged CreateFile encoding remains replayable as well.
        let created = drive
            .mutate(
                "file-create",
                "actor",
                Mutation::CreateFile {
                    parent: NodeId(1),
                    name: "file".into(),
                    content_type: "text/plain".into(),
                    bytes: b"persisted-file".to_vec(),
                },
            )
            .unwrap();
        assert_eq!(created.node.last_mutation_id, "file-create");
        assert_eq!(
            drive.collect(None, 100).unwrap().removed,
            0,
            "replay must not write blobs"
        );
        drop(drive);
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert_eq!(drive.list(NodeId(1), None, 100).unwrap().nodes, before);
        assert!(matches!(
            drive.metadata(NodeId(3)),
            Err(crate::Error::NotFound)
        ));
    }

    #[test]
    fn migrated_receipts_reject_changed_actor_payload_or_precondition_without_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_update(&path, false);
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        let before = drive.list(NodeId(1), None, 100).unwrap().nodes;
        for (request, mutation) in changed_replays() {
            let status = drive.request_status(request).unwrap();
            assert!(matches!(
                drive.mutate(request, "other-actor", mutation.clone()),
                Err(crate::Error::Conflict)
            ));
            let mut changed = mutation.clone();
            match &mut changed {
                Mutation::Update { bytes, .. } => *bytes = b"changed".to_vec(),
                Mutation::Relocate { name, .. } => *name = "changed".into(),
                Mutation::Delete { id, .. } => *id = NodeId(2),
                _ => unreachable!(),
            }
            assert!(matches!(
                drive.mutate(request, "actor", changed),
                Err(crate::Error::Conflict)
            ));
            let mut changed = mutation;
            match &mut changed {
                Mutation::Update {
                    expected_mutation_id,
                    ..
                }
                | Mutation::Relocate {
                    expected_mutation_id,
                    ..
                }
                | Mutation::Delete {
                    expected_mutation_id,
                    ..
                } => *expected_mutation_id = "wrong-request".into(),
                _ => unreachable!(),
            }
            assert!(matches!(
                drive.mutate(request, "actor", changed),
                Err(crate::Error::Conflict)
            ));
            assert_eq!(drive.request_status(request).unwrap(), status);
        }
        assert_eq!(drive.list(NodeId(1), None, 100).unwrap().nodes, before);
        assert_eq!(drive.collect(None, 100).unwrap().removed, 0);
    }

    #[test]
    fn missing_prior_receipt_requires_status_recovery_and_never_reexecutes_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_update(&path, true);
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        let before = drive.list(NodeId(1), None, 100).unwrap().nodes;
        for (request, mutation) in changed_replays().into_iter().take(2) {
            let status = drive.request_status(request).unwrap();
            assert!(matches!(status, RequestStatus::Committed { .. }));
            assert!(matches!(
                drive.mutate(request, "actor", mutation.clone()),
                Err(crate::Error::Conflict)
            ));
            // Even substituting the *current* guard cannot reexecute an old identity.
            let mut changed = mutation;
            match &mut changed {
                Mutation::Update {
                    expected_mutation_id,
                    bytes,
                    ..
                } => {
                    *expected_mutation_id = request.into();
                    *bytes = b"changed".to_vec();
                }
                Mutation::Relocate {
                    expected_mutation_id,
                    name,
                    ..
                } => {
                    *expected_mutation_id = request.into();
                    *name = "changed".into();
                }
                _ => unreachable!(),
            }
            assert!(matches!(
                drive.mutate(request, "actor", changed),
                Err(crate::Error::Conflict)
            ));
            assert_eq!(drive.request_status(request).unwrap(), status);
        }
        assert_eq!(drive.list(NodeId(1), None, 100).unwrap().nodes, before);
        assert_eq!(drive.collect(None, 100).unwrap().removed, 0);
    }

    #[test]
    fn migrated_receipt_evidence_does_not_prevent_workspace_purge() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("metadata");
        seed_update(&path, false);
        let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        drive.purge().unwrap();
        drop(drive);
        let drive = Drive::open(&storage, &registration, &temp.path().join("blobs")).unwrap();
        assert!(drive.list(NodeId(1), None, 100).unwrap().nodes.is_empty());
        for (request, _) in changed_replays() {
            assert_eq!(
                drive.request_status(request).unwrap(),
                RequestStatus::Uncommitted
            );
        }
        drive
            .database
            .try_with_connection::<_, feature_storage::FeatureStorageError>(|c| {
                let remaining: i64 =
                    c.query_row("SELECT count(*) FROM drive_legacy_replays", [], |r| {
                        r.get(0)
                    })?;
                assert_eq!(remaining, 0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn missing_or_ambiguous_receipts_abort_upgrade_without_partial_schema_or_data_changes() {
        for copies in [0, 2] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("metadata");
            seed_old(&path, copies);
            {
                let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
                let registration = Drive::register(&storage).unwrap();
                assert!(storage.open(&registration).is_err());
            }
            let storage = FeatureStorage::new(&path).workspace("ws").unwrap();
            let old = storage
                .register(FeatureRegistration::new(
                    "workspace-drive",
                    &MIGRATIONS[..1],
                ))
                .unwrap();
            let database = storage.open(&old).unwrap();
            assert_eq!(database.schema_version().unwrap(), 1);
            database.try_with_connection::<_, feature_storage::FeatureStorageError>(|c| {
                let row: (String, i64) = c.query_row("SELECT name,revision FROM drive_nodes WHERE id=2", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
                assert_eq!(row, ("after".into(), 2));
                let added: i64 = c.query_row("SELECT count(*) FROM pragma_table_info('drive_nodes') WHERE name='last_mutation_id'", [], |r| r.get(0))?;
                assert_eq!(added, 0);
                let receipt: String = c.query_row("SELECT result_json FROM drive_receipts WHERE request_id='create'", [], |r| r.get(0))?;
                assert_eq!(serde_json::from_str::<serde_json::Value>(&receipt).unwrap()["node"]["revision"], "1");
                Ok(())
            }).unwrap();
        }
    }
}
