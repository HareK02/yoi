//! File-backed SQLite tests of the owner boundary, using production receipt,
//! plan and catalog DDL. The minimal Workspace parent is a fixture, not a second
//! migration path. Parent migration transition/rollback/retry tests live upstream.
use super::*;
use crate::plugin_plan::{
    ConfigProvenance, PlanPinState, PluginPinProjection, PluginSurface, ProfileProvenance,
    WorkerPluginExecutionPlan, WorkerPluginPin, WorkerPluginPlanInput,
};
use rusqlite::TransactionBehavior;
use server_api::{PluginArtifact, PluginSchemaContribution, PluginToolMetadata};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::{TempDir, tempdir};

const NOW: i64 = 1000;
const RETENTION: i64 = 500;
const INSTALLATION: &str = "installation";

fn fingerprint(seed: &str) -> String {
    let hex: String = Sha256::digest(seed.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}
fn consumer(id: &str) -> PluginConsumer {
    PluginConsumer::ToolCall {
        tool_call_id: id.into(),
        tool_name: "example_echo".into(),
    }
}
fn response() -> HttpResult {
    Ok(PluginHttpResponse {
        status: 503, // HTTP failure is still an observed response, not transport error.
        headers: vec![
            ("x-result".into(), "one".into()),
            ("x-result".into(), "two".into()),
        ],
        body: vec![0, 255, 128, 13, 10],
    })
}
fn open(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .unwrap();
    conn.busy_timeout(Duration::from_secs(10)).unwrap();
    conn
}
struct Fixture {
    dir: TempDir,
    plan: StoredPluginPlan,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("receipts.db"));
        conn.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY); INSERT INTO workspaces VALUES('a'),('b');").unwrap();
        conn.execute_batch(include_str!("../plugin_catalog_schema.sql"))
            .unwrap();
        conn.execute_batch(crate::plugin_plan::SCHEMA_SQL).unwrap();
        conn.execute_batch(SCHEMA_SQL).unwrap();
        for ws in ["a", "b"] {
            conn.execute(
                "INSERT INTO plugin_installations VALUES(?1,?2,'alias','package','operation',0)",
                params![ws, INSTALLATION],
            )
            .unwrap();
        }
        let plan = make_plan("plan", "config", "a", "runtime", "worker", "principal");
        save_plan(&conn, &plan);
        Self { dir, plan }
    }
    fn path(&self) -> std::path::PathBuf {
        self.dir.path().join("receipts.db")
    }
    fn conn(&self) -> Connection {
        open(&self.path())
    }
    fn begin(&self, id: &str) -> InvocationRecord {
        let mut conn = self.conn();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let record = begin(&tx, &self.plan, INSTALLATION, &consumer(id), NOW, RETENTION).unwrap();
        tx.commit().unwrap();
        record
    }
    fn write<T>(&self, work: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = self.conn();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let value = work(&tx)?;
        tx.commit().unwrap();
        Ok(value)
    }
    fn admit(
        &self,
        record: &InvocationRecord,
        key: &str,
        seed: &str,
        now: i64,
    ) -> Result<EffectAdmission> {
        self.write(|tx| effect_admit(tx, record, key, &fingerprint(seed), now))
    }
}
fn make_plan(
    id: &str,
    config: &str,
    workspace: &str,
    runtime: &str,
    worker: &str,
    principal: &str,
) -> StoredPluginPlan {
    let artifact = PluginArtifact {
        package_id: "package".into(),
        name: "package".into(),
        version: "1.0.0".into(),
        description: None,
        package_digest: fingerprint("package"),
        artifact_digest: fingerprint("artifact"),
        artifact_size: 1,
        schema_version: manifest::plugin::SUPPORTED_PLUGIN_API_VERSION,
        world: manifest::plugin::PLUGIN_COMPONENT_TOOL_WORLD.into(),
        required_imports: vec![],
        tools: vec![PluginToolMetadata {
            name: "example_echo".into(),
            description: "echo".into(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            error_codes: vec![],
            external_write: false,
        }],
        connections: vec![],
        requested_grants: vec![],
        data_collections: vec![],
        config_schema: PluginSchemaContribution {
            identity: "schema".into(),
            version: "1".into(),
            digest: fingerprint("schema"),
            namespace: "package".into(),
            dcdl: "".into(),
        },
        revoked: false,
        revocation_reason: None,
    };
    let pin = WorkerPluginPin::from_artifact(
        INSTALLATION.into(),
        "operation".into(),
        &artifact,
        PluginPinProjection {
            config: serde_json::json!({}),
            surfaces: vec![PluginSurface::Tool],
            tools: vec!["example_echo".into()],
            grants: vec![],
            bindings: vec![],
        },
    )
    .unwrap();
    let plan = WorkerPluginExecutionPlan::canonicalize(WorkerPluginPlanInput {
        workspace_id: workspace.into(),
        config: ConfigProvenance {
            content_digest: fingerprint(config),
            toolchain_fingerprint: "toolchain".into(),
        },
        profile: ProfileProvenance {
            identity: "profile".into(),
            projection_digest: fingerprint("profile"),
        },
        execution: HostExecutionIdentity {
            runtime_id: runtime.into(),
            worker_id: worker.into(),
            principal_id: principal.into(),
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
fn save_plan(conn: &Connection, plan: &StoredPluginPlan) {
    conn.execute("INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES(?1,?2,'fixture',?2,?3,?4,'active')",
        params![plan.plan.workspace_id(), plan.plan_id, plan.fingerprint, plan.plan.canonical_json().unwrap()]).unwrap();
}
fn error<T: std::fmt::Debug>(result: Result<T>, code: PluginHostErrorCode) -> PluginHostError {
    match result {
        Err(Error::PluginHost(error)) if error.code == code => error,
        other => panic!("expected {code:?}, got {other:?}"),
    }
}
fn replay_unknown(result: Result<EffectAdmission>) -> PluginHostError {
    match result.unwrap() {
        EffectAdmission::Replay(Err(error)) => {
            assert_eq!(error.code, PluginHostErrorCode::Indeterminate);
            assert_eq!(error.effect, PluginEffectObservation::Indeterminate);
            error
        }
        other => panic!("must never redispatch pending/unknown: {other:?}"),
    }
}

#[test]
fn begin_replay_reopen_preserves_uuid_and_original_deadline_and_conflicts_on_input() {
    let f = Fixture::new();
    let first = f.begin("call");
    uuid::Uuid::parse_str(&first.invocation_id).unwrap();
    let again = f
        .write(|tx| {
            begin(
                tx,
                &f.plan,
                INSTALLATION,
                &consumer("call"),
                NOW + 100,
                RETENTION * 10,
            )
        })
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(first.expires_at_ms, NOW + RETENTION);
    assert_eq!(lookup(&f.conn(), "a", &first.invocation_id).unwrap(), first);
    error(
        lookup(&f.conn(), "b", &first.invocation_id),
        PluginHostErrorCode::NotFound,
    );
    let other_plan = make_plan(
        "different-plan",
        "config",
        "a",
        "runtime",
        "worker",
        "principal",
    );
    save_plan(&f.conn(), &other_plan);
    error(
        f.write(|tx| {
            begin(
                tx,
                &other_plan,
                INSTALLATION,
                &consumer("call"),
                NOW,
                RETENTION,
            )
        }),
        PluginHostErrorCode::Conflict,
    );
    let other_input = make_plan(
        "changed-input",
        "new-config",
        "a",
        "runtime",
        "worker",
        "principal",
    );
    save_plan(&f.conn(), &other_input);
    error(
        f.write(|tx| {
            begin(
                tx,
                &other_input,
                INSTALLATION,
                &consumer("call"),
                NOW,
                RETENTION,
            )
        }),
        PluginHostErrorCode::Conflict,
    );
    let renamed = PluginConsumer::ToolCall {
        tool_call_id: "call".into(),
        tool_name: "different".into(),
    };
    error(
        f.write(|tx| begin(tx, &f.plan, INSTALLATION, &renamed, NOW, RETENTION)),
        PluginHostErrorCode::Conflict,
    );
    let changed_execution = make_plan(
        "principal-change",
        "config",
        "a",
        "runtime",
        "worker",
        "other-principal",
    );
    save_plan(&f.conn(), &changed_execution);
    error(
        f.write(|tx| {
            begin(
                tx,
                &changed_execution,
                INSTALLATION,
                &consumer("call"),
                NOW,
                RETENTION,
            )
        }),
        PluginHostErrorCode::Conflict,
    );
    assert_ne!(f.begin("new-call").invocation_id, first.invocation_id);
}

#[test]
fn identities_isolate_workspaces_runtimes_workers_installations_and_consumers() {
    let f = Fixture::new();
    let original = f.begin("same-id");
    let mut ids = std::collections::BTreeSet::from([original.invocation_id]);
    for (id, ws, runtime, worker) in [
        ("other-workspace", "b", "runtime", "worker"),
        ("other-runtime", "a", "runtime-2", "worker"),
        ("other-worker", "a", "runtime", "worker-2"),
    ] {
        let plan = make_plan(id, "config", ws, runtime, worker, "principal");
        save_plan(&f.conn(), &plan);
        let record = f
            .write(|tx| {
                begin(
                    tx,
                    &plan,
                    INSTALLATION,
                    &consumer("same-id"),
                    NOW,
                    RETENTION,
                )
            })
            .unwrap();
        assert!(ids.insert(record.invocation_id));
    }
    // Another immutable installation selection gets an independent namespace.
    let mut plan_json = serde_json::to_value(&f.plan.plan).unwrap();
    plan_json["input"]["plugins"][0]["installation_id"] = serde_json::json!("other-installation");
    let other_snapshot: WorkerPluginExecutionPlan = serde_json::from_value(plan_json).unwrap();
    let other = StoredPluginPlan {
        plan_id: "other-installation-plan".into(),
        fingerprint: other_snapshot.fingerprint().unwrap(),
        pin_state: PlanPinState::Active,
        plan: other_snapshot,
    };
    f.conn().execute("INSERT INTO plugin_installations VALUES('a','other-installation','other-alias','package','operation',0)", []).unwrap();
    save_plan(&f.conn(), &other);
    let record = f
        .write(|tx| {
            begin(
                tx,
                &other,
                "other-installation",
                &consumer("same-id"),
                NOW,
                RETENTION,
            )
        })
        .unwrap();
    assert!(ids.insert(record.invocation_id));
    for consumer_id in ["consumer-1", "consumer-2"] {
        let c = PluginConsumer::Consumer {
            consumer_id: consumer_id.into(),
            event_id: "same-id".into(),
            export_name: "export".into(),
        };
        let record = f
            .write(|tx| begin(tx, &f.plan, INSTALLATION, &c, NOW, RETENTION))
            .unwrap();
        assert!(ids.insert(record.invocation_id.clone()));
        assert_eq!(
            f.write(|tx| begin(tx, &f.plan, INSTALLATION, &c, NOW + 5, RETENTION))
                .unwrap(),
            record
        );
        let renamed = PluginConsumer::Consumer {
            consumer_id: consumer_id.into(),
            event_id: "same-id".into(),
            export_name: "other".into(),
        };
        error(
            f.write(|tx| begin(tx, &f.plan, INSTALLATION, &renamed, NOW, RETENTION)),
            PluginHostErrorCode::Conflict,
        );
    }
    // Missing installation cannot open a namespace absent from the immutable plan.
    error(
        f.write(|tx| {
            begin(
                tx,
                &f.plan,
                "not-in-plan",
                &consumer("same-id"),
                NOW,
                RETENTION,
            )
        }),
        PluginHostErrorCode::InvalidInput,
    );
}

#[test]
fn two_connections_racing_admission_commit_exactly_one_dispatch() {
    let f = Fixture::new();
    let record = f.begin("race");
    let barrier = Arc::new(Barrier::new(2));
    let sends = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let (path, record, barrier, sends) =
                (f.path(), record.clone(), barrier.clone(), sends.clone());
            std::thread::spawn(move || {
                let mut conn = open(&path);
                barrier.wait();
                let tx = conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .unwrap();
                let admission =
                    effect_admit(&tx, &record, "GET-key", &fingerprint("GET /"), NOW).unwrap();
                tx.commit().unwrap();
                if admission == EffectAdmission::Dispatch {
                    sends.fetch_add(1, Ordering::SeqCst);
                }
                admission
            })
        })
        .collect();
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|a| **a == EffectAdmission::Dispatch)
            .count(),
        1
    );
    assert!(outcomes.iter().any(|a| matches!(a, EffectAdmission::Replay(Err(error)) if error.code == PluginHostErrorCode::Indeterminate)));
    error(
        f.admit(&record, "GET-key", "POST /", NOW),
        PluginHostErrorCode::Conflict,
    );
}

#[test]
fn crash_after_durable_admission_reopens_pending_with_zero_redispatch() {
    let f = Fixture::new();
    let record = f.begin("crash");
    let sends = AtomicUsize::new(0);
    if f.admit(&record, "key", "request", NOW).unwrap() == EffectAdmission::Dispatch {
        sends.fetch_add(1, Ordering::SeqCst);
    }
    // All helpers close their connections. Simulate crash before any completion.
    let saved_json: String = f
        .conn()
        .query_row("SELECT error_json FROM plugin_effect_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    let saved: PluginHostError = serde_json::from_str(&saved_json).unwrap();
    for _ in 0..3 {
        let recovered = lookup(&f.conn(), "a", &record.invocation_id).unwrap();
        assert_eq!(
            replay_unknown(f.admit(&recovered, "key", "request", NOW + 1)),
            saved
        );
    }
    assert_eq!(sends.load(Ordering::SeqCst), 1);
}

#[test]
fn completion_replays_exact_binary_response_headers_and_http_status_without_dispatch() {
    let f = Fixture::new();
    let record = f.begin("response");
    assert_eq!(
        f.admit(&record, "key", "request", NOW).unwrap(),
        EffectAdmission::Dispatch
    );
    f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &response()))
        .unwrap();
    assert_eq!(
        f.admit(&record, "key", "request", NOW).unwrap(),
        EffectAdmission::Replay(response())
    );
    // Successful completion replay is a read, not an overwrite.
    f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &response()))
        .unwrap();
    error(
        f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("different"), &response())),
        PluginHostErrorCode::Conflict,
    );
    let changed = Ok(PluginHttpResponse {
        status: 200,
        headers: vec![],
        body: vec![],
    });
    error(
        f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &changed)),
        PluginHostErrorCode::Conflict,
    );
    error(
        f.write(|tx| {
            effect_complete(
                tx,
                &record,
                "never-admitted",
                &fingerprint("request"),
                &changed,
            )
        }),
        PluginHostErrorCode::NotFound,
    );
}

#[test]
fn completion_saves_transport_failure_for_each_observation_and_never_redispatches() {
    let f = Fixture::new();
    let record = f.begin("errors");
    for (key, observation) in [
        ("before", PluginEffectObservation::NotDispatched),
        ("after", PluginEffectObservation::Indeterminate),
        ("observed", PluginEffectObservation::ResponseObserved),
    ] {
        assert_eq!(
            f.admit(&record, key, key, NOW).unwrap(),
            EffectAdmission::Dispatch
        );
        let failure = Err(PluginHostError {
            code: PluginHostErrorCode::DeadlineExceeded,
            effect: observation,
            diagnostic_id: format!("diagnostic-{key}"),
        });
        f.write(|tx| effect_complete(tx, &record, key, &fingerprint(key), &failure))
            .unwrap();
        assert_eq!(
            f.admit(&record, key, key, NOW + 1).unwrap(),
            EffectAdmission::Replay(failure)
        );
    }
}

#[test]
fn cancellation_is_irreversible_and_stale_records_cannot_bypass_it() {
    let f = Fixture::new();
    let record = f.begin("cancel");
    assert_eq!(
        f.admit(&record, "in-flight", "request", NOW).unwrap(),
        EffectAdmission::Dispatch
    );
    f.write(|tx| cancel(tx, "a", &record.invocation_id))
        .unwrap();
    f.write(|tx| cancel(tx, "a", &record.invocation_id))
        .unwrap();
    assert!(
        lookup(&f.conn(), "a", &record.invocation_id)
            .unwrap()
            .cancelled
    );
    for key in ["in-flight", "new-key"] {
        let e = error(
            f.admit(&record, key, "request", NOW),
            PluginHostErrorCode::Cancelled,
        );
        assert_eq!(e.effect, PluginEffectObservation::Indeterminate);
    }
    // Recording an observed in-flight response does not publish it after cancel.
    f.write(|tx| {
        effect_complete(
            tx,
            &record,
            "in-flight",
            &fingerprint("request"),
            &response(),
        )
    })
    .unwrap();
    error(
        f.admit(&record, "in-flight", "request", NOW),
        PluginHostErrorCode::Cancelled,
    );
    assert!(
        f.conn()
            .execute("UPDATE plugin_invocations SET cancelled=0", [])
            .is_err()
    );
    assert!(f.begin("cancel").cancelled);
}

#[test]
fn deadline_expiry_refuses_old_and_new_keys_and_prune_preserves_tombstones() {
    let f = Fixture::new();
    let record = f.begin("expire");
    let untouched = f.begin("no-effects");
    for key in ["complete", "pending"] {
        assert_eq!(
            f.admit(&record, key, "request", NOW).unwrap(),
            EffectAdmission::Dispatch
        );
    }
    f.write(|tx| {
        effect_complete(
            tx,
            &record,
            "complete",
            &fingerprint("request"),
            &response(),
        )
    })
    .unwrap();
    for key in ["complete", "pending", "unused"] {
        error(
            f.admit(&record, key, "request", NOW + RETENTION),
            PluginHostErrorCode::ReceiptExpired,
        );
    }
    error(
        f.admit(&untouched, "unused", "request", NOW + RETENTION),
        PluginHostErrorCode::ReceiptExpired,
    );
    let recovered = f
        .write(|tx| {
            begin(
                tx,
                &f.plan,
                INSTALLATION,
                &consumer("expire"),
                NOW + RETENTION + 10,
                RETENTION * 100,
            )
        })
        .unwrap();
    assert_eq!(record, recovered);
    assert_eq!(f.write(|tx| prune(tx, "a", NOW + RETENTION)).unwrap(), 2);
    assert_eq!(f.write(|tx| prune(tx, "a", NOW + RETENTION)).unwrap(), 0);
    let conn = f.conn();
    let counts: (i64, i64, i64) = conn.query_row("SELECT count(*),sum(state='expired'),sum(response_body IS NULL AND response_headers IS NULL AND error_json IS NULL) FROM plugin_effect_receipts", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(counts, (2, 2, 2));
    // Explicit pruning is monotonic even if the caller's clock moves backwards.
    error(
        f.admit(&record, "unused", "request", NOW),
        PluginHostErrorCode::ReceiptExpired,
    );
    error(
        f.admit(&untouched, "unused", "request", NOW),
        PluginHostErrorCode::ReceiptExpired,
    );
    error(
        f.write(|tx| {
            effect_complete(
                tx,
                &record,
                "complete",
                &fingerprint("request"),
                &response(),
            )
        }),
        PluginHostErrorCode::ReceiptExpired,
    );
    assert!(
        conn.execute("UPDATE plugin_invocations SET expired=0", [])
            .is_err()
    );
    assert_eq!(
        lookup(&conn, "a", &record.invocation_id).unwrap(),
        InvocationRecord {
            expired: true,
            ..record
        }
    );
    assert!(
        conn.execute("DELETE FROM plugin_effect_receipts", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM plugin_invocations", []).is_err());
    assert!(
        conn.execute("UPDATE plugin_effect_receipts SET state='pending'", [])
            .is_err()
    );
    // Workspace deletion is the only identity/key purge. All FK checks remain on.
    conn.execute("DELETE FROM workspaces WHERE workspace_id='a'", [])
        .unwrap();
    for table in ["plugin_invocations", "plugin_effect_receipts"] {
        let count: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let fk_errors: i64 = conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(fk_errors, 0);
}

#[test]
fn response_and_key_bounds_reject_without_erasing_indeterminate_receipt() {
    let f = Fixture::new();
    let record = f.begin("bounds");
    assert_eq!(
        f.admit(&record, "key", "request", NOW).unwrap(),
        EffectAdmission::Dispatch
    );
    for body_len in [MAX_RESPONSE_BYTES + 1] {
        let big = Ok(PluginHttpResponse {
            status: 200,
            headers: vec![],
            body: vec![255; body_len],
        });
        error(
            f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &big)),
            PluginHostErrorCode::QuotaExceeded,
        );
        replay_unknown(f.admit(&record, "key", "request", NOW));
    }
    let too_many_headers = Ok(PluginHttpResponse {
        status: 200,
        headers: vec![("x".into(), "y".into()); MAX_RESPONSE_HEADERS + 1],
        body: vec![],
    });
    error(
        f.write(|tx| {
            effect_complete(
                tx,
                &record,
                "key",
                &fingerprint("request"),
                &too_many_headers,
            )
        }),
        PluginHostErrorCode::QuotaExceeded,
    );
    let big_headers = Ok(PluginHttpResponse {
        status: 200,
        headers: vec![("x".into(), "y".repeat(MAX_RESPONSE_HEADER_BYTES))],
        body: vec![],
    });
    error(
        f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &big_headers)),
        PluginHostErrorCode::QuotaExceeded,
    );
    for key in [
        "".to_owned(),
        "x".repeat(129),
        "é".repeat(65),
        "bad\0key".into(),
    ] {
        error(
            f.admit(&record, &key, "request", NOW),
            PluginHostErrorCode::InvalidInput,
        );
    }
    error(
        f.write(|tx| effect_admit(tx, &record, "key", "raw request must not be stored", NOW)),
        PluginHostErrorCode::InvalidInput,
    );
    let max = Ok(PluginHttpResponse {
        status: 200,
        headers: vec![],
        body: vec![255; MAX_RESPONSE_BYTES],
    });
    f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &max))
        .unwrap();
    assert_eq!(
        f.admit(&record, "key", "request", NOW).unwrap(),
        EffectAdmission::Replay(max)
    );
}

#[test]
fn schema_guards_identity_result_overwrites_replace_and_unadmitted_completion() {
    let f = Fixture::new();
    let record = f.begin("guards");
    f.admit(&record, "key", "request", NOW).unwrap();
    f.write(|tx| effect_complete(tx, &record, "key", &fingerprint("request"), &response()))
        .unwrap();
    let conn = f.conn();
    // REPLACE protection must not depend on PRAGMA recursive_triggers.
    conn.execute_batch("PRAGMA recursive_triggers=OFF;")
        .unwrap();
    for sql in [
        "DELETE FROM plugin_invocations",
        "DELETE FROM plugin_effect_receipts",
        "UPDATE plugin_invocations SET invocation_id='replacement'",
        "UPDATE plugin_invocations SET expires_at_ms=99999",
        "UPDATE plugin_invocations SET consumer_json='{}'",
        "UPDATE plugin_effect_receipts SET fingerprint='sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
        "UPDATE plugin_effect_receipts SET response_body=X'01'",
        "UPDATE plugin_effect_receipts SET state='pending',response_status=NULL,response_headers=NULL,response_body=NULL,error_json='{}'",
        "UPDATE plugin_effect_receipts SET state='expired',response_status=NULL,response_headers=NULL,response_body=NULL,error_json=NULL",
        "INSERT OR REPLACE INTO plugin_invocations SELECT * FROM plugin_invocations",
        "INSERT OR REPLACE INTO plugin_effect_receipts SELECT * FROM plugin_effect_receipts",
        "INSERT INTO plugin_effect_receipts SELECT workspace_id,invocation_id,'new-key',fingerprint,admitted_at_ms,state,response_status,response_headers,response_body,error_json FROM plugin_effect_receipts",
    ] {
        assert!(conn.execute(sql, []).is_err(), "must reject {sql}");
    }
    let mut forged = record.clone();
    forged.expires_at_ms += 1000;
    error(
        f.admit(&forged, "new-key", "request", NOW),
        PluginHostErrorCode::Conflict,
    );
    forged = record.clone();
    forged.execution.worker_id = "another-worker".into();
    error(
        f.admit(&forged, "new-key", "request", NOW),
        PluginHostErrorCode::Conflict,
    );
    assert_eq!(
        f.admit(&record, "key", "request", NOW).unwrap(),
        EffectAdmission::Replay(response())
    );
}

#[test]
fn uncommitted_admission_rolls_back_and_failed_prune_is_atomic() {
    let f = Fixture::new();
    let record = f.begin("rollback");
    {
        let mut conn = f.conn();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert_eq!(
            effect_admit(&tx, &record, "key", &fingerprint("request"), NOW).unwrap(),
            EffectAdmission::Dispatch
        );
        // No send: commit was not successful. Transaction rollback is safe here.
    }
    assert_eq!(
        f.admit(&record, "key", "request", NOW).unwrap(),
        EffectAdmission::Dispatch
    );
    let mut conn = f.conn();
    conn.execute_batch("CREATE TRIGGER refuse_prune BEFORE UPDATE ON plugin_effect_receipts WHEN NEW.state='expired' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert!(prune(&tx, "a", NOW + RETENTION).is_err());
    tx.commit().unwrap();
    let expired: bool = conn
        .query_row("SELECT expired FROM plugin_invocations", [], |r| r.get(0))
        .unwrap();
    assert!(
        !expired,
        "caller catching an error must not commit partial expiry"
    );
    replay_unknown(f.admit(&record, "key", "request", NOW));
}

#[test]
fn invalid_begin_does_not_create_identity_or_overflow_deadline() {
    let f = Fixture::new();
    for (now, retention) in [(-1, 1), (NOW, 0), (NOW, -1), (i64::MAX, 1)] {
        error(
            f.write(|tx| {
                begin(
                    tx,
                    &f.plan,
                    INSTALLATION,
                    &consumer("invalid"),
                    now,
                    retention,
                )
            }),
            PluginHostErrorCode::InvalidInput,
        );
    }
    let invalid = PluginConsumer::ToolCall {
        tool_call_id: "".into(),
        tool_name: "tool".into(),
    };
    error(
        f.write(|tx| begin(tx, &f.plan, INSTALLATION, &invalid, NOW, RETENTION)),
        PluginHostErrorCode::InvalidInput,
    );
    let mut plan = f.plan.clone();
    plan.fingerprint = fingerprint("different");
    error(
        f.write(|tx| {
            begin(
                tx,
                &plan,
                INSTALLATION,
                &consumer("invalid"),
                NOW,
                RETENTION,
            )
        }),
        PluginHostErrorCode::InvalidInput,
    );
    let count: i64 = f
        .conn()
        .query_row("SELECT count(*) FROM plugin_invocations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn all_http_methods_including_get_and_head_have_the_same_one_shot_receipt_semantics() {
    let f = Fixture::new();
    let record = f.begin("all-methods");
    let sends = AtomicUsize::new(0);
    for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
        if f.admit(&record, method, method, NOW).unwrap() == EffectAdmission::Dispatch {
            sends.fetch_add(1, Ordering::SeqCst);
        }
        replay_unknown(f.admit(&record, method, method, NOW));
        error(
            f.admit(&record, method, "changed-method-or-body", NOW),
            PluginHostErrorCode::Conflict,
        );
        f.write(|tx| effect_complete(tx, &record, method, &fingerprint(method), &response()))
            .unwrap();
        assert_eq!(
            f.admit(&record, method, method, NOW).unwrap(),
            EffectAdmission::Replay(response())
        );
    }
    assert_eq!(sends.load(Ordering::SeqCst), 7);
}

#[test]
fn schema_checks_initial_indeterminate_result_and_bounded_payloads_independently_of_rust() {
    let f = Fixture::new();
    let record = f.begin("sql-checks");
    let conn = f.conn();
    for json in [
        "{}",
        r#"{"code":"unavailable","effect":"not_dispatched","diagnostic_id":"d"}"#,
    ] {
        assert!(conn.execute("INSERT INTO plugin_effect_receipts(workspace_id,invocation_id,effect_key,fingerprint,admitted_at_ms,state,error_json) VALUES('a',?1,'invalid-pending',?2,1000,'pending',?3)", params![record.invocation_id, fingerprint("request"), json]).is_err());
    }
    assert!(conn.execute("INSERT INTO plugin_effect_receipts(workspace_id,invocation_id,effect_key,fingerprint,admitted_at_ms,state,error_json) VALUES('a',?1,'expired-admission',?2,1500,'pending',?3)", params![record.invocation_id, fingerprint("request"), encode(&host_error(PluginHostErrorCode::Indeterminate, PluginEffectObservation::Indeterminate)).unwrap()]).is_err());
    f.admit(&record, "key", "request", NOW).unwrap();
    for sql in [
        "UPDATE plugin_effect_receipts SET state='completed',response_status=200,response_headers='[]',response_body=zeroblob(1048577),error_json=NULL",
        "UPDATE plugin_effect_receipts SET state='completed',response_status=200,response_headers='[]',response_body='not-a-blob',error_json=NULL",
        "UPDATE plugin_effect_receipts SET state='completed',response_status=99,response_headers='[]',response_body=X'',error_json=NULL",
        "UPDATE plugin_effect_receipts SET state='completed',response_status=200,response_headers='{}',response_body=X'',error_json=NULL",
    ] {
        assert!(conn.execute(sql, []).is_err(), "must reject {sql}");
    }
    replay_unknown(f.admit(&record, "key", "request", NOW));
}

#[test]
fn pruning_one_workspace_does_not_remove_another_workspaces_saved_result() {
    let f = Fixture::new();
    let a = f.begin("prune-scope");
    let plan_b = make_plan("plan-b", "config", "b", "runtime", "worker", "principal");
    save_plan(&f.conn(), &plan_b);
    let b = f
        .write(|tx| {
            begin(
                tx,
                &plan_b,
                INSTALLATION,
                &consumer("prune-scope"),
                NOW,
                RETENTION,
            )
        })
        .unwrap();
    for record in [&a, &b] {
        f.admit(record, "key", "request", NOW).unwrap();
        f.write(|tx| effect_complete(tx, record, "key", &fingerprint("request"), &response()))
            .unwrap();
    }
    assert_eq!(f.write(|tx| prune(tx, "a", NOW + RETENTION)).unwrap(), 1);
    assert_eq!(
        f.admit(&b, "key", "request", NOW).unwrap(),
        EffectAdmission::Replay(response())
    );
    error(
        f.admit(&a, "key", "request", NOW),
        PluginHostErrorCode::ReceiptExpired,
    );
    // Unexpired and pending invocations are not accidentally pruned early.
    assert_eq!(
        f.write(|tx| prune(tx, "b", NOW + RETENTION - 1)).unwrap(),
        0
    );
}
