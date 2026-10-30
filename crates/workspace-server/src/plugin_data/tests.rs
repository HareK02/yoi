//! Real file-backed SQLite owner tests: production catalog/plan/invocation/data
//! DDL and functions, independent connections, fixed deadlines and bounded thread
//! synchronization. Only Workspace/Ticket parent rows and live authorization are
//! fixtures; this suite does not claim guest authorization or adapter coverage.
use super::*;
use crate::plugin_plan::{
    ConfigProvenance, HostExecutionIdentity, PlanPinState, PluginPinProjection, PluginSurface,
    ProfileProvenance, StoredPluginPlan, WorkerPluginExecutionPlan, WorkerPluginPin,
    WorkerPluginPlanInput,
};
use rusqlite::TransactionBehavior;
use server_api::{PluginArtifact, PluginConsumer};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Barrier, mpsc},
    time::Duration,
};
use tempfile::{TempDir, tempdir};

const NOW: i64 = 1000;
const RETENTION: i64 = 500;
fn open(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .unwrap();
    conn.busy_timeout(Duration::from_secs(10)).unwrap();
    conn
}
fn artifact() -> PluginArtifact {
    serde_json::from_value(serde_json::json!({
        "package_id":"package","name":"package","version":"1.0.0","description":null,
        "package_digest":hash(b"package"),"artifact_digest":hash(b"artifact"),"artifact_size":1,
        "schema_version":2,"world":manifest::plugin::PLUGIN_COMPONENT_TOOL_WORLD,
        "required_imports":[],"tools":[{"name":"echo","description":"echo","input_schema":{},"output_schema":{},"error_codes":[],"external_write":false}],
        "connections":[],"requested_grants":[],"data_collections":[],
        "config_schema":{"identity":"schema","version":"1","namespace":"package","digest":hash(b"schema"),"dcdl":""},
        "revoked":false,"revocation_reason":null
    })).unwrap()
}
fn plan(workspace: &str, installation: &str, id: &str) -> StoredPluginPlan {
    let pin = WorkerPluginPin::from_artifact(
        installation.into(),
        "catalog-op".into(),
        &artifact(),
        PluginPinProjection {
            config: serde_json::json!({}),
            surfaces: vec![PluginSurface::Tool],
            tools: vec!["echo".into()],
            grants: vec![],
            bindings: vec![],
        },
    )
    .unwrap();
    let plan = WorkerPluginExecutionPlan::canonicalize(WorkerPluginPlanInput {
        workspace_id: workspace.into(),
        config: ConfigProvenance {
            content_digest: hash(b"config"),
            toolchain_fingerprint: "toolchain".into(),
        },
        profile: ProfileProvenance {
            identity: "profile".into(),
            projection_digest: hash(b"profile"),
        },
        execution: HostExecutionIdentity {
            runtime_id: "runtime".into(),
            worker_id: "worker".into(),
            principal_id: "principal".into(),
            delegation_id: Some("delegation".into()),
        },
        plugins: vec![pin],
    })
    .unwrap();
    StoredPluginPlan {
        plan_id: id.into(),
        fingerprint: plan.fingerprint().unwrap(),
        pin_state: PlanPinState::Active,
        plan,
    }
}
struct Fixture {
    dir: TempDir,
    record: InvocationRecord,
    reference: PluginReference,
    ns: Namespace,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let path = dir.path().join("data.db");
        let mut conn = open(&path);
        conn.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY,state TEXT NOT NULL); INSERT INTO workspaces VALUES('a','active'),('b','active'); CREATE TABLE typed_tickets(workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,PRIMARY KEY(workspace_id,ticket_id)); INSERT INTO typed_tickets VALUES('a','ticket'),('a','other-ticket'),('b','ticket');").unwrap();
        conn.execute_batch(include_str!("../plugin_catalog_schema.sql"))
            .unwrap();
        conn.execute_batch(crate::plugin_plan::SCHEMA_SQL).unwrap();
        conn.execute_batch(plugin_receipt::SCHEMA_SQL).unwrap();
        // The production published baseline is upgraded by NEW migration SQL.
        conn.execute_batch(REFERENCE_MIGRATION_SQL).unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        let artifact = artifact();
        for workspace in ["a", "b"] {
            for installation in ["installation", "other-installation"] {
                conn.execute(
                    "INSERT INTO plugin_installations VALUES(?1,?2,?2,'package','catalog-op',0)",
                    params![workspace, installation],
                )
                .unwrap();
                conn.execute("INSERT INTO plugin_artifacts(workspace_id,installation_id,artifact_digest,package_digest,version,artifact_size,metadata_json) VALUES(?1,?2,?3,?4,'1.0.0',1,?5)",params![workspace,installation,artifact.artifact_digest,artifact.package_digest,encode(&artifact).unwrap()]).unwrap();
            }
        }
        let stored = plan("a", "installation", "plan");
        conn.execute("INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES('a','plan','fixture','plan',?1,?2,'active')",params![stored.fingerprint,stored.plan.canonical_json().unwrap()]).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let record = plugin_receipt::begin(
            &tx,
            &stored,
            "installation",
            &PluginConsumer::ToolCall {
                tool_call_id: "call".into(),
                tool_name: "echo".into(),
            },
            NOW,
            RETENTION,
        )
        .unwrap();
        tx.commit().unwrap();
        Self {
            dir,
            record,
            reference: PluginReference {
                kind: PluginReferenceKind::Data,
                owner: "ignored".into(),
                installation_id: "installation".into(),
                expected_last_operation: "catalog-op".into(),
                package_digest: artifact.package_digest,
                artifact_digest: artifact.artifact_digest,
            },
            ns: Namespace {
                workspace_id: "a".into(),
                installation_id: "installation".into(),
                ticket_id: "ticket".into(),
                collection: PluginDataCollection {
                    collection: "collection".into(),
                    schema_id: "schema".into(),
                    schema_version: "1".into(),
                },
            },
        }
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("data.db")
    }
    fn conn(&self) -> Connection {
        open(&self.path())
    }
    fn write<T>(&self, work: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = work(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    fn put(
        &self,
        key: &str,
        operation: &str,
        expected: PluginDataExpectation,
        value: &str,
    ) -> Result<PluginDataReceipt> {
        self.write(|tx| {
            mutate(
                tx,
                &self.record,
                &self.ns,
                key,
                operation,
                &PluginDataMutation::Put {
                    expected,
                    value_json: value.into(),
                },
                &self.reference,
                NOW,
            )
        })
    }
    fn revoke(&self) {
        self.write(|tx| {
            tx.execute("UPDATE plugin_artifacts SET revoked=1,revocation_reason='admin' WHERE workspace_id='a' AND installation_id='installation'",[])?;
            tx.execute("UPDATE plugin_installations SET last_operation='revoke-op' WHERE workspace_id='a' AND installation_id='installation'",[])?;
            Ok(())
        }).unwrap();
    }
    fn count(&self, table: &str) -> i64 {
        self.conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}
fn expect_error<T: std::fmt::Debug>(result: Result<T>, code: PluginHostErrorCode) {
    assert!(
        matches!(&result,Err(Error::PluginHost(error)) if error.code==code),
        "expected {code:?}: {result:?}"
    );
}
fn put(expected: PluginDataExpectation, value: &str) -> PluginDataMutation {
    PluginDataMutation::Put {
        expected,
        value_json: value.into(),
    }
}

#[test]
fn lost_response_reopen_replays_before_cas_and_rejects_changed_input() {
    let f = Fixture::new();
    let first = f
        .put(
            "key",
            "write",
            PluginDataExpectation::NeverWritten,
            "{\"b\":2,\"a\":1}",
        )
        .unwrap();
    let later = f
        .put(
            "key",
            "later",
            PluginDataExpectation::LastWrite(first.mutation_id.clone()),
            "{}",
        )
        .unwrap();
    // Fresh connection, lost first response: canonical equivalent input replays.
    let replay = f
        .put(
            "key",
            "write",
            PluginDataExpectation::NeverWritten,
            "{\"a\":1,\"b\":2}",
        )
        .unwrap();
    assert_eq!(first, replay);
    assert_ne!(first, later);
    assert!(
        matches!(get(&f.conn(),&f.ns,"key").unwrap(),PluginDataCell::Present{last_write,..} if last_write==later.mutation_id)
    );
    assert_eq!(
        lookup_mutation(&f.conn(), &f.record, &f.ns, "key", "write", NOW).unwrap(),
        PluginDataMutationLookup::Committed(first)
    );
    expect_error(
        f.put("key", "write", PluginDataExpectation::NeverWritten, "null"),
        PluginHostErrorCode::Conflict,
    );
    expect_error(
        f.put(
            "different",
            "write",
            PluginDataExpectation::NeverWritten,
            "{}",
        ),
        PluginHostErrorCode::Conflict,
    );
    expect_error(
        lookup_mutation(&f.conn(), &f.record, &f.ns, "different", "write", NOW),
        PluginHostErrorCode::Conflict,
    );
    assert_eq!(
        lookup_mutation(&f.conn(), &f.record, &f.ns, "key", "unused", NOW).unwrap(),
        PluginDataMutationLookup::NotRecorded
    );
}

#[test]
fn delete_tombstone_prevents_never_written_and_stale_operation_aba() {
    let f = Fixture::new();
    assert_eq!(
        get(&f.conn(), &f.ns, "key").unwrap(),
        PluginDataCell::NeverWritten
    );
    let first = f
        .put("key", "put", PluginDataExpectation::NeverWritten, "null")
        .unwrap();
    let deleted = f
        .write(|tx| {
            mutate(
                tx,
                &f.record,
                &f.ns,
                "key",
                "delete",
                &PluginDataMutation::Delete {
                    expected_last_write: first.mutation_id.clone(),
                },
                &f.reference,
                NOW,
            )
        })
        .unwrap();
    assert_eq!(
        get(&f.conn(), &f.ns, "key").unwrap(),
        PluginDataCell::Deleted {
            last_delete: deleted.mutation_id.clone()
        }
    );
    expect_error(
        f.put(
            "key",
            "stale-never",
            PluginDataExpectation::NeverWritten,
            "null",
        ),
        PluginHostErrorCode::Conflict,
    );
    expect_error(
        f.put(
            "key",
            "stale-put",
            PluginDataExpectation::LastWrite(first.mutation_id),
            "null",
        ),
        PluginHostErrorCode::Conflict,
    );
    let recreated = f
        .put(
            "key",
            "recreate",
            PluginDataExpectation::LastWrite(deleted.mutation_id),
            "null",
        )
        .unwrap();
    assert!(
        matches!(get(&f.conn(),&f.ns,"key").unwrap(),PluginDataCell::Present{last_write,value_json} if value_json=="null" && last_write==recreated.mutation_id)
    );
    assert_eq!(f.count("plugin_references"), 1);
}

#[test]
fn namespace_schema_rollback_and_cursor_rebinding_cannot_cross_cells() {
    let f = Fixture::new();
    f.put("a", "a", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    f.put("b", "b", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    let page = list(&f.conn(), &f.ns, None, 1, NOW).unwrap();
    assert_eq!(page.entries[0].key, "a");
    let cursor = page.next_cursor.unwrap();
    let mut scopes = vec![];
    let mut ns = f.ns.clone();
    ns.workspace_id = "b".into();
    scopes.push(ns);
    let mut ns = f.ns.clone();
    ns.installation_id = "other-installation".into();
    scopes.push(ns);
    let mut ns = f.ns.clone();
    ns.ticket_id = "other-ticket".into();
    scopes.push(ns);
    let mut ns = f.ns.clone();
    ns.collection.collection = "other".into();
    scopes.push(ns);
    let mut ns = f.ns.clone();
    ns.collection.schema_id = "other".into();
    scopes.push(ns);
    let mut ns = f.ns.clone();
    ns.collection.schema_version = "2".into();
    scopes.push(ns.clone());
    for scope in &scopes {
        assert_eq!(
            get(&f.conn(), scope, "a").unwrap(),
            PluginDataCell::NeverWritten
        );
        expect_error(
            list(&f.conn(), scope, Some(&cursor), 1, NOW),
            PluginHostErrorCode::InvalidInput,
        );
    }
    f.write(|tx| {
        mutate(
            tx,
            &f.record,
            &ns,
            "a",
            "schema-two",
            &put(PluginDataExpectation::NeverWritten, "{\"v\":2}"),
            &f.reference,
            NOW,
        )
    })
    .unwrap();
    assert!(
        matches!(get(&f.conn(),&f.ns,"a").unwrap(),PluginDataCell::Present{value_json,..} if value_json=="{}")
    );
    assert!(
        matches!(get(&f.conn(),&ns,"a").unwrap(),PluginDataCell::Present{value_json,..} if value_json=="{\"v\":2}")
    );
    let next = list(&f.conn(), &f.ns, Some(&cursor), 1, NOW).unwrap();
    assert_eq!(next.entries[0].key, "b");
    assert!(next.next_cursor.is_none());
    expect_error(
        list(&f.conn(), &f.ns, Some(&cursor), 1, NOW + CURSOR_LIFETIME_MS),
        PluginHostErrorCode::InvalidInput,
    );
    expect_error(
        list(&f.conn(), &f.ns, Some("malformed"), 1, NOW),
        PluginHostErrorCode::InvalidInput,
    );
    for limit in [0, 101] {
        expect_error(
            list(&f.conn(), &f.ns, None, limit, NOW),
            PluginHostErrorCode::InvalidInput,
        );
    }
}

#[test]
fn expiry_and_prune_preserve_keys_and_cannot_resurrect_on_backwards_clock() {
    let f = Fixture::new();
    f.put(
        "key",
        "write",
        PluginDataExpectation::NeverWritten,
        "{\"secret\":true}",
    )
    .unwrap();
    assert_eq!(
        lookup_mutation(
            &f.conn(),
            &f.record,
            &f.ns,
            "key",
            "unused",
            NOW + RETENTION
        )
        .unwrap(),
        PluginDataMutationLookup::Expired
    );
    f.write(|tx| {
        plugin_receipt::prune(tx, "a", NOW + RETENTION)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        lookup_mutation(&f.conn(), &f.record, &f.ns, "key", "write", NOW).unwrap(),
        PluginDataMutationLookup::Expired
    );
    expect_error(
        f.put("new", "new", PluginDataExpectation::NeverWritten, "{}"),
        PluginHostErrorCode::ReceiptExpired,
    );
    assert_eq!(f.count("plugin_data_receipts"), 1);
    let payload: Option<String> = f
        .conn()
        .query_row("SELECT result_json FROM plugin_data_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(payload.is_none());
    assert!(
        f.conn()
            .execute("DELETE FROM plugin_data_receipts", [])
            .is_err()
    );
    assert!(matches!(
        get(&f.conn(), &f.ns, "key").unwrap(),
        PluginDataCell::Present { .. }
    ));
}

#[test]
fn invocation_identity_and_cancellation_are_rechecked_from_owner() {
    let f = Fixture::new();
    let mut forged = f.record.clone();
    forged.consumer = PluginConsumer::ToolCall {
        tool_call_id: "forged".into(),
        tool_name: "echo".into(),
    };
    expect_error(
        f.write(|tx| {
            mutate(
                tx,
                &forged,
                &f.ns,
                "key",
                "op",
                &put(PluginDataExpectation::NeverWritten, "{}"),
                &f.reference,
                NOW,
            )
        }),
        PluginHostErrorCode::Conflict,
    );
    let mut foreign = f.ns.clone();
    foreign.workspace_id = "b".into();
    expect_error(
        f.write(|tx| {
            mutate(
                tx,
                &f.record,
                &foreign,
                "key",
                "op",
                &put(PluginDataExpectation::NeverWritten, "{}"),
                &f.reference,
                NOW,
            )
        }),
        PluginHostErrorCode::Conflict,
    );
    f.write(|tx| plugin_receipt::cancel(tx, "a", &f.record.invocation_id))
        .unwrap();
    expect_error(
        f.put("key", "op", PluginDataExpectation::NeverWritten, "{}"),
        PluginHostErrorCode::Cancelled,
    );
    expect_error(
        lookup_mutation(&f.conn(), &f.record, &f.ns, "key", "unused", NOW),
        PluginHostErrorCode::Cancelled,
    );
    assert_eq!(f.count("plugin_data_cells"), 0);
}

#[test]
fn receipt_failure_rolls_back_cell_reference_and_holder_even_if_owner_commits() {
    let f = Fixture::new();
    f.conn().execute_batch("CREATE TRIGGER inject_receipt_failure BEFORE INSERT ON plugin_data_receipts BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").unwrap();
    f.write(|tx| {
        assert!(
            mutate(
                tx,
                &f.record,
                &f.ns,
                "key",
                "write",
                &put(PluginDataExpectation::NeverWritten, "{}"),
                &f.reference,
                NOW
            )
            .is_err()
        );
        tx.execute(
            "UPDATE workspaces SET state='active' WHERE workspace_id='b'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    for table in [
        "plugin_data_cells",
        "plugin_data_holders",
        "plugin_data_receipts",
        "plugin_references",
    ] {
        assert_eq!(f.count(table), 0, "{table}");
    }
}

#[test]
fn http_and_data_share_operation_key_namespace_without_a_second_invocation() {
    let f = Fixture::new();
    f.put("key", "data", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    expect_error(
        f.write(|tx| plugin_receipt::effect_admit(tx, &f.record, "data", &hash(b"http"), NOW)),
        PluginHostErrorCode::Conflict,
    );
    f.write(|tx| plugin_receipt::effect_admit(tx, &f.record, "http", &hash(b"http"), NOW))
        .unwrap();
    expect_error(
        f.put("other", "http", PluginDataExpectation::NeverWritten, "{}"),
        PluginHostErrorCode::Conflict,
    );
    expect_error(
        lookup_mutation(&f.conn(), &f.record, &f.ns, "key", "http", NOW),
        PluginHostErrorCode::Conflict,
    );
    assert_eq!(f.count("plugin_invocations"), 1);
}

#[test]
fn concurrent_final_write_cas_has_one_committed_winner() {
    let f = Fixture::new();
    let gate = Arc::new(Barrier::new(3));
    let handles:Vec<_>=(0..2).map(|index| {
        let gate=gate.clone(); let path=f.path(); let record=f.record.clone(); let ns=f.ns.clone(); let reference=f.reference.clone();
        std::thread::spawn(move || {
            let mut conn=open(&path); gate.wait();
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
            let outcome=mutate(&tx,&record,&ns,"key",&format!("writer-{index}"),&put(PluginDataExpectation::NeverWritten,"{}"),&reference,NOW);
            match outcome { Ok(receipt)=>{tx.commit().unwrap();Some(receipt)},Err(error)=>{assert!(matches!(error,Error::PluginHost(ref e) if e.code==PluginHostErrorCode::Conflict));None} }
        })
    }).collect();
    gate.wait();
    let winners: Vec<_> = handles
        .into_iter()
        .filter_map(|h| h.join().unwrap())
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(f.count("plugin_data_receipts"), 1);
    assert!(
        matches!(get(&f.conn(),&f.ns,"key").unwrap(),PluginDataCell::Present{last_write,..} if last_write==winners[0].mutation_id)
    );
}

#[test]
fn physical_ticket_deletion_and_workspace_deleting_fence_stale_writers() {
    let f = Fixture::new();
    let mut deleting = f.conn();
    let tx = deleting
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "DELETE FROM typed_tickets WHERE workspace_id='a' AND ticket_id='ticket'",
        [],
    )
    .unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let path = f.path();
    let record = f.record.clone();
    let ns = f.ns.clone();
    let reference = f.reference.clone();
    let writer = std::thread::spawn(move || {
        let mut conn = open(&path);
        started_tx.send(()).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        mutate(
            &tx,
            &record,
            &ns,
            "key",
            "late",
            &put(PluginDataExpectation::NeverWritten, "{}"),
            &reference,
            NOW,
        )
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    tx.commit().unwrap();
    expect_error(writer.join().unwrap(), PluginHostErrorCode::NotFound);
    assert_eq!(f.count("plugin_data_cells"), 0);
    // The opposite ordering retains data but no read/write may resolve deleted parent.
    let f = Fixture::new();
    f.put("key", "first", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    f.write(|tx| {
        tx.execute(
            "DELETE FROM typed_tickets WHERE workspace_id='a' AND ticket_id='ticket'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    expect_error(get(&f.conn(), &f.ns, "key"), PluginHostErrorCode::NotFound);
    assert_eq!(f.count("plugin_data_cells"), 1);
    let f = Fixture::new();
    f.write(|tx| {
        tx.execute(
            "UPDATE workspaces SET state='deleting' WHERE workspace_id='a'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    expect_error(
        f.put("key", "late", PluginDataExpectation::NeverWritten, "{}"),
        PluginHostErrorCode::NotFound,
    );
    // Physical SQL fences are not merely advisory owner checks.
    assert!(f.conn().execute("INSERT INTO plugin_data_cells VALUES('a','installation','ticket','collection','schema','1','key','illegal','{}')",[]).is_err());
}

#[test]
fn uninstall_racing_first_data_write_cannot_drop_retained_installation() {
    let f = Fixture::new();
    let gate = Arc::new(Barrier::new(3));
    let path = f.path();
    let gate_writer = gate.clone();
    let record = f.record.clone();
    let ns = f.ns.clone();
    let reference = f.reference.clone();
    let writer = std::thread::spawn(move || {
        let mut conn = open(&path);
        gate_writer.wait();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let success = mutate(
            &tx,
            &record,
            &ns,
            "key",
            "write",
            &put(PluginDataExpectation::NeverWritten, "{}"),
            &reference,
            NOW,
        )
        .is_ok();
        tx.commit().unwrap();
        success
    });
    let path = f.path();
    let gate_uninstall = gate.clone();
    let uninstall = std::thread::spawn(move || {
        let mut conn = open(&path);
        gate_uninstall.wait();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let success=tx.execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='a' AND installation_id='installation'",[]).is_ok();
        tx.commit().unwrap();
        success
    });
    gate.wait();
    let wrote = writer.join().unwrap();
    let uninstalled = uninstall.join().unwrap();
    assert_ne!(wrote, uninstalled);
    assert_eq!(f.count("plugin_data_cells"), if wrote { 1 } else { 0 });
}

#[test]
fn purge_requires_post_revocation_exact_set_and_retains_replay_after_uninstall() {
    let f = Fixture::new();
    let first = f
        .put("key", "first", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    expect_error(
        f.write(|tx| purge_snapshot(tx, "a", "installation")),
        PluginHostErrorCode::Conflict,
    );
    let deleted = f
        .write(|tx| {
            mutate(
                tx,
                &f.record,
                &f.ns,
                "key",
                "delete",
                &PluginDataMutation::Delete {
                    expected_last_write: first.mutation_id,
                },
                &f.reference,
                NOW,
            )
        })
        .unwrap();
    f.revoke();
    assert_eq!(f.count("plugin_data_cells"), 1);
    let snapshot = f
        .write(|tx| purge_snapshot(tx, "a", "installation"))
        .unwrap();
    assert_eq!(snapshot.cell_count, 1);
    // A missing tombstone is a different exact set, not an opaque version counter.
    let mut wrong = snapshot.clone();
    wrong.cells_digest = hash(deleted.mutation_id.as_bytes());
    expect_error(
        f.write(|tx| purge(tx, "a", "installation", "admin", "purge", &wrong)),
        PluginHostErrorCode::Conflict,
    );
    assert_eq!(f.count("plugin_references"), 1);
    let receipt = f
        .write(|tx| purge(tx, "a", "installation", "admin", "purge", &snapshot))
        .unwrap();
    assert_eq!(f.count("plugin_data_cells"), 0);
    assert_eq!(f.count("plugin_data_holders"), 0);
    assert_eq!(f.count("plugin_references"), 0);
    assert_eq!(f.count("plugin_data_receipts"), 2);
    assert_eq!(f.count("plugin_data_purges"), 1);
    f.write(|tx| {tx.execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='a' AND installation_id='installation'",[])?;Ok(())}).unwrap();
    assert_eq!(
        f.write(|tx| purge(tx, "a", "installation", "admin", "purge", &snapshot))
            .unwrap(),
        receipt
    );
    expect_error(
        f.write(|tx| purge(tx, "a", "installation", "admin", "purge", &wrong)),
        PluginHostErrorCode::Conflict,
    );
    assert!(
        f.conn()
            .execute("DELETE FROM plugin_data_purges", [])
            .is_err()
    );
}

#[test]
fn purge_rollback_keeps_cells_and_references_and_no_success_receipt() {
    let f = Fixture::new();
    f.put("key", "write", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    f.revoke();
    let snapshot = f
        .write(|tx| purge_snapshot(tx, "a", "installation"))
        .unwrap();
    f.conn().execute_batch("CREATE TRIGGER inject_reference_failure BEFORE DELETE ON plugin_references BEGIN SELECT RAISE(ABORT,'injected purge failure'); END;").unwrap();
    f.write(|tx| {
        assert!(purge(tx, "a", "installation", "admin", "purge", &snapshot).is_err());
        Ok(())
    })
    .unwrap();
    assert_eq!(f.count("plugin_data_cells"), 1);
    assert_eq!(f.count("plugin_data_holders"), 1);
    assert_eq!(f.count("plugin_references"), 1);
    assert_eq!(f.count("plugin_data_purges"), 0);
}

#[test]
fn purge_serializes_with_uninstall_and_parent_deletion_without_receipt_loss() {
    let f = Fixture::new();
    f.put("key", "write", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    f.revoke();
    // Uninstall cannot win before purge removes retained Data reference.
    assert!(f.conn().execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='a' AND installation_id='installation'",[]).is_err());
    let mut owner = f.conn();
    let tx = owner
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let snapshot = purge_snapshot(&tx, "a", "installation").unwrap();
    let receipt = purge(&tx, "a", "installation", "admin", "purge", &snapshot).unwrap();
    let path = f.path();
    let (started_tx, started_rx) = mpsc::channel();
    let deleting = std::thread::spawn(move || {
        let mut conn = open(&path);
        started_tx.send(()).unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "DELETE FROM typed_tickets WHERE workspace_id='a' AND ticket_id='ticket'",
            [],
        )
        .unwrap();
        tx.execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='a' AND installation_id='installation'",[]).unwrap();
        tx.commit().unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    tx.commit().unwrap();
    deleting.join().unwrap();
    assert_eq!(
        f.write(|tx| purge(tx, "a", "installation", "admin", "purge", &snapshot))
            .unwrap(),
        receipt
    );
    assert_eq!(f.count("plugin_data_cells"), 0);
    assert_eq!(f.count("plugin_data_purges"), 1);
}

#[test]
fn new_reference_migration_preserves_published_rows_and_failed_upgrade_rolls_back() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY); INSERT INTO workspaces VALUES('w');").unwrap();
    conn.execute_batch(include_str!("../plugin_catalog_schema.sql"))
        .unwrap();
    conn.execute(
        "INSERT INTO plugin_installations VALUES('w','i','alias','package','op',0)",
        [],
    )
    .unwrap();
    let a = hash(b"artifact");
    let p = hash(b"package");
    conn.execute("INSERT INTO plugin_artifacts(workspace_id,installation_id,artifact_digest,package_digest,version,artifact_size,metadata_json) VALUES('w','i',?1,?2,'1',1,'{}')",params![a,p]).unwrap();
    conn.execute(
        "INSERT INTO plugin_references VALUES('w','active','owner','i',?1)",
        [&a],
    )
    .unwrap();
    {
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute_batch(REFERENCE_MIGRATION_SQL).unwrap();
        tx.execute_batch("CREATE TABLE plugin_data_cells(collision INTEGER);")
            .unwrap();
        assert!(tx.execute_batch(SCHEMA_SQL).is_err());
        tx.rollback().unwrap();
    }
    assert!(
        conn.execute(
            "INSERT INTO plugin_references VALUES('w','data','owner','i',?1)",
            [&a]
        )
        .is_err()
    );
    plugin_catalog::verify(&conn).unwrap();
    conn.execute_batch(REFERENCE_MIGRATION_SQL).unwrap();
    plugin_catalog::verify_with_data(&conn).unwrap();
    let row: (String, String) = conn
        .query_row("SELECT kind,owner FROM plugin_references", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(row, ("active".into(), "owner".into()));
    conn.execute(
        "INSERT INTO plugin_references VALUES('w','data','data-owner','i',?1)",
        [&a],
    )
    .unwrap();
    assert!(
        conn.execute(
            "UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='w'",
            []
        )
        .is_err()
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}

#[test]
fn purge_catalog_fence_change_conflicts_even_when_exact_cell_set_is_unchanged() {
    let f = Fixture::new();
    f.put("key", "write", PluginDataExpectation::NeverWritten, "{}")
        .unwrap();
    f.revoke();
    let snapshot = f
        .write(|tx| purge_snapshot(tx, "a", "installation"))
        .unwrap();
    // A catalog operation (e.g. adding then revoking an artifact) changes the
    // execution fence even though no typed cells were written in between.
    f.write(|tx| {
        tx.execute("UPDATE plugin_installations SET last_operation='another-catalog-op' WHERE workspace_id='a' AND installation_id='installation'", [])?;
        Ok(())
    }).unwrap();
    expect_error(
        f.write(|tx| purge(tx, "a", "installation", "admin", "purge", &snapshot)),
        PluginHostErrorCode::Conflict,
    );
    assert_eq!(f.count("plugin_data_cells"), 1);
    assert_eq!(f.count("plugin_data_purges"), 0);
    let current = f
        .write(|tx| purge_snapshot(tx, "a", "installation"))
        .unwrap();
    assert_eq!(snapshot.cells_digest, current.cells_digest);
    assert_ne!(
        snapshot.installation_last_operation,
        current.installation_last_operation
    );
}

#[test]
fn maximum_value_and_key_bounds_are_enforced_without_partial_records() {
    let f = Fixture::new();
    let maximum_key = "k".repeat(256);
    // Backslash-heavy JSON expands when encoded in a receipt's value_json field.
    let value = format!(
        "\"{}\"",
        "\\\\".repeat((server_api::PLUGIN_DATA_VALUE_LIMIT - 2) / 2)
    );
    assert_eq!(value.len(), server_api::PLUGIN_DATA_VALUE_LIMIT);
    f.put(
        &maximum_key,
        "write",
        PluginDataExpectation::NeverWritten,
        &value,
    )
    .unwrap();
    assert!(
        matches!(get(&f.conn(), &f.ns, &maximum_key).unwrap(), PluginDataCell::Present { value_json, .. } if value_json == value)
    );
    for key in [String::new(), "k".repeat(257), "bad\nkey".into()] {
        expect_error(
            f.put(&key, "invalid", PluginDataExpectation::NeverWritten, "{}"),
            PluginHostErrorCode::InvalidInput,
        );
    }
    expect_error(
        f.put(
            "oversized",
            "oversized",
            PluginDataExpectation::NeverWritten,
            &format!("\"{}\"", "x".repeat(65536)),
        ),
        PluginHostErrorCode::QuotaExceeded,
    );
    expect_error(
        f.put(
            "invalid",
            "invalid",
            PluginDataExpectation::NeverWritten,
            "not-json",
        ),
        PluginHostErrorCode::InvalidInput,
    );
    assert_eq!(f.count("plugin_data_cells"), 1);
    assert_eq!(f.count("plugin_data_receipts"), 1);
}

#[test]
fn opaque_cursor_reopen_tampering_and_deadline_extension_are_rejected() {
    let f = Fixture::new();
    for key in ["a", "b", "c"] {
        f.put(key, key, PluginDataExpectation::NeverWritten, "{}")
            .unwrap();
    }
    let first = list(&f.conn(), &f.ns, None, 1, NOW).unwrap();
    let token = first.next_cursor.unwrap();
    assert_eq!(token.len(), 68);
    assert!(token.starts_with("pdc-"));
    assert!(serde_json::from_str::<serde_json::Value>(&token).is_err());
    let mut tampered = token.clone();
    let replacement = if token.ends_with('0') { "1" } else { "0" };
    tampered.replace_range(67..68, replacement);
    expect_error(
        list(&f.conn(), &f.ns, Some(&tampered), 1, NOW),
        PluginHostErrorCode::InvalidInput,
    );
    let forged_json = encode(&serde_json::json!({"namespace":digest(&f.ns).unwrap(),"after":"a","expires_at_ms":NOW+CURSOR_LIFETIME_MS})).unwrap();
    expect_error(
        list(&f.conn(), &f.ns, Some(&forged_json), 1, NOW),
        PluginHostErrorCode::InvalidInput,
    );
    // Independent reopened connection recovers the selector, not client data.
    let second = list(&f.conn(), &f.ns, Some(&token), 1, NOW + 50).unwrap();
    assert_eq!(second.entries[0].key, "b");
    let next = second.next_cursor.unwrap();
    let deadline: i64 = f
        .conn()
        .query_row(
            "SELECT expires_at_ms FROM plugin_data_cursors WHERE cursor_token=?1",
            [&next],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deadline, NOW + CURSOR_LIFETIME_MS);
    expect_error(
        list(&f.conn(), &f.ns, Some(&next), 1, deadline),
        PluginHostErrorCode::InvalidInput,
    );
    // Autocommit expiry pruning forgets the opaque token permanently.
    expect_error(
        list(&f.conn(), &f.ns, Some(&next), 1, NOW),
        PluginHostErrorCode::InvalidInput,
    );
}

#[test]
fn cursor_selector_map_is_bounded_reusable_and_expired_rows_are_reclaimed() {
    let f = Fixture::new();
    for key in ["a", "b", "c"] {
        f.put(key, key, PluginDataExpectation::NeverWritten, "{}")
            .unwrap();
    }
    let token = list(&f.conn(), &f.ns, None, 1, NOW)
        .unwrap()
        .next_cursor
        .unwrap();
    f.write(|tx| {
        let mut insert=tx.prepare("INSERT INTO plugin_data_cursors(workspace_id,installation_id,cursor_token,namespace_digest,after_key,expires_at_ms) VALUES('a','installation',?1,?2,'offset',?3)")?;
        for index in 1..MAX_CURSORS_PER_WORKSPACE {
            insert.execute(params![format!("pdc-{index:064x}"),hash(format!("fixture-scope-{index}").as_bytes()),NOW+CURSOR_LIFETIME_MS])?;
        }
        Ok(())
    }).unwrap();
    assert_eq!(f.count("plugin_data_cursors"), MAX_CURSORS_PER_WORKSPACE);
    // Reissuing the exact existing selector needs no additional capacity.
    assert_eq!(
        list(&f.conn(), &f.ns, None, 1, NOW)
            .unwrap()
            .next_cursor
            .unwrap(),
        token
    );
    expect_error(
        list(&f.conn(), &f.ns, Some(&token), 1, NOW),
        PluginHostErrorCode::QuotaExceeded,
    );
    let fresh = list(&f.conn(), &f.ns, None, 1, NOW + CURSOR_LIFETIME_MS)
        .unwrap()
        .next_cursor
        .unwrap();
    assert_ne!(fresh, token);
    assert_eq!(f.count("plugin_data_cursors"), 1);
}

#[test]
fn explicit_purge_invalidates_persisted_pagination_selectors() {
    let f = Fixture::new();
    for key in ["a", "b"] {
        f.put(key, key, PluginDataExpectation::NeverWritten, "{}")
            .unwrap();
    }
    let token = list(&f.conn(), &f.ns, None, 1, NOW)
        .unwrap()
        .next_cursor
        .unwrap();
    assert_eq!(f.count("plugin_data_cursors"), 1);
    f.revoke();
    let snapshot = f
        .write(|tx| purge_snapshot(tx, "a", "installation"))
        .unwrap();
    f.write(|tx| purge(tx, "a", "installation", "admin", "purge", &snapshot))
        .unwrap();
    assert_eq!(f.count("plugin_data_cursors"), 0);
    expect_error(
        list(&f.conn(), &f.ns, Some(&token), 1, NOW),
        PluginHostErrorCode::InvalidInput,
    );
}
