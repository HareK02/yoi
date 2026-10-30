//! Real SQLite tests of catalog artifact registration and immutable typed schema
//! namespaces. Manifest/Component validation and caller authorization are outside
//! this owner test; immutable metadata DTOs are the input boundary. No guest runs.
use super::*;
use std::{
    sync::{Arc, Barrier},
    time::Duration as StdDuration,
};
use tempfile::tempdir;

fn artifact(version: &str, schema_version: &str, schema: serde_json::Value) -> PluginArtifact {
    serde_json::from_value(serde_json::json!({
        "package_id":"package","name":"package","version":version,"description":null,
        "package_digest":format!("sha256:{}",hash(format!("package-{version}").as_bytes())),
        "artifact_digest":format!("sha256:{}",hash(format!("artifact-{version}").as_bytes())),"artifact_size":1,
        "schema_version":2,"world":manifest::plugin::PLUGIN_COMPONENT_TICKET_TOOL_WORLD,
        "required_imports":[],"tools":[],"connections":[],"requested_grants":[],
        "data_collections":[{"collection":"collection","schema_id":"schema","schema_version":schema_version,"schema":schema}],
        "config_schema":{"identity":"config","version":"1","namespace":"package","digest":format!("sha256:{}",hash(b"config")),"dcdl":""},
        "revoked":false,"revocation_reason":null
    })).unwrap()
}
fn schema(value_type: &str) -> serde_json::Value {
    serde_json::json!({"type":"object","properties":{"value":{"type":value_type}},"required":["value"],"additionalProperties":false})
}
fn open(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")
        .unwrap();
    conn.busy_timeout(StdDuration::from_secs(10)).unwrap();
    conn
}
fn initialize(conn: &Connection) {
    conn.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY); INSERT INTO workspaces VALUES('w'),('other-workspace');").unwrap();
    conn.execute_batch(include_str!("../plugin_catalog_schema.sql"))
        .unwrap();
    for workspace in ["w", "other-workspace"] {
        for installation in ["i", "other-installation"] {
            conn.execute(
                "INSERT INTO plugin_installations VALUES(?1,?2,?2,'package','op',0)",
                params![workspace, installation],
            )
            .unwrap();
        }
    }
}
fn register(
    conn: &mut Connection,
    workspace: &str,
    installation: &str,
    candidate: &PluginArtifact,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    add_artifact(&tx, workspace, installation, candidate)?;
    tx.commit()?;
    Ok(())
}
fn conflict(result: Result<()>) {
    assert!(
        matches!(
            result,
            Err(Error::PluginCatalog(PluginCatalogError::Conflict))
        ),
        "expected schema conflict, got {result:?}"
    );
}

#[test]
fn registered_schema_content_is_immutable_per_namespace_including_revoked_artifacts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let mut conn = open(&path);
    initialize(&conn);
    let first = artifact("1.0.0", "1", schema("string"));
    register(&mut conn, "w", "i", &first).unwrap();
    // Different object member order is the same canonical schema, not a change.
    let reordered=serde_json::from_str(r#"{"additionalProperties":false,"required":["value"],"properties":{"value":{"type":"string"}},"type":"object"}"#).unwrap();
    register(&mut conn, "w", "i", &artifact("1.0.1", "1", reordered)).unwrap();
    conn.execute("UPDATE plugin_artifacts SET revoked=1,revocation_reason='admin' WHERE workspace_id='w' AND installation_id='i'",[]).unwrap();
    drop(conn);
    let mut conn = open(&path);
    let changed = artifact("2.0.0", "1", schema("integer"));
    conflict(register(&mut conn, "w", "i", &changed));
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM plugin_artifacts WHERE workspace_id='w' AND installation_id='i'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    // Explicit new schema version registers a separate namespace; rollback keeps
    // the original schema metadata instead of reading it as the new definition.
    let next = artifact("2.0.0", "2", schema("integer"));
    register(&mut conn, "w", "i", &next).unwrap();
    let saved = load_installation(&conn, "w", "i").unwrap();
    let old = saved
        .artifacts
        .iter()
        .find(|a| a.version == "1.0.0")
        .unwrap();
    let new = saved
        .artifacts
        .iter()
        .find(|a| a.version == "2.0.0")
        .unwrap();
    assert!(old.revoked);
    assert_eq!(old.data_collections[0].schema_version, "1");
    assert_eq!(old.data_collections[0].schema, schema("string"));
    assert_eq!(new.data_collections[0].schema_version, "2");
    assert_eq!(new.data_collections[0].schema, schema("integer"));
    // Installation and Workspace ownership are independent schema namespaces.
    register(&mut conn, "w", "other-installation", &changed).unwrap();
    register(&mut conn, "other-workspace", "i", &changed).unwrap();
}

#[test]
fn concurrent_artifact_registration_cannot_redefine_a_schema_namespace() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    initialize(&open(&path));
    let gate = Arc::new(Barrier::new(3));
    let handles: Vec<_> = [("1.0.0", "string"), ("2.0.0", "integer")]
        .into_iter()
        .map(|(version, kind)| {
            let path = path.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                let mut conn = open(&path);
                gate.wait();
                let result = register(&mut conn, "w", "i", &artifact(version, "1", schema(kind)));
                match result {
                    Ok(()) => true,
                    Err(error) => {
                        assert!(matches!(
                            error,
                            Error::PluginCatalog(PluginCatalogError::Conflict)
                        ));
                        false
                    }
                }
            })
        })
        .collect();
    gate.wait();
    let winners = handles
        .into_iter()
        .map(|handle| usize::from(handle.join().unwrap()))
        .sum::<usize>();
    assert_eq!(winners, 1);
    assert_eq!(
        load_installation(&open(&path), "w", "i")
            .unwrap()
            .artifacts
            .len(),
        1
    );
}
