//! Frozen schema-135 launch authorization conversion. Old ordinals are resolved
//! through the saved authorization-changing operation, not renamed into counters.
//! Creation fingerprints remain immutable evidence; no request preimage is stored
//! from which a different public wire request could be reconstructed safely.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const MIGRATION_NAME: &str = "Launch policy authorization operation identities";
type Identities = BTreeMap<(String, String, u64), String>;
fn invalid(message: impl std::fmt::Display) -> Error {
    Error::Store(format!("Launch policy identity migration: {message}"))
}
fn parse(raw: &str) -> Result<Value> {
    serde_json::from_str(raw).map_err(invalid)
}
fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("missing {field}")))
}
fn key(map: &Identities, workspace: &str, policy: &str, value: &Value) -> Result<String> {
    let ordinal = value
        .as_u64()
        .filter(|v| (1..=9_007_199_254_740_991).contains(v))
        .ok_or_else(|| invalid("invalid legacy authorization identity"))?;
    map.get(&(workspace.into(), policy.into(), ordinal))
        .cloned()
        .ok_or_else(|| invalid("authorization-changing operation is missing"))
}
fn convert_identity(
    value: &mut Value,
    map: &Identities,
    workspace: &str,
    policy: &str,
    old: &str,
    new: &str,
) -> Result<()> {
    if value.get(new).is_some() {
        return Err(invalid("mixed old and current identities"));
    }
    let converted = key(
        map,
        workspace,
        policy,
        value
            .get(old)
            .ok_or_else(|| invalid("missing saved authorization identity"))?,
    )?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| invalid("identity is not an object"))?;
    object.remove(old);
    object.insert(new.into(), Value::String(converted));
    Ok(())
}
fn policy_json(raw: &str, map: &Identities, workspace: &str, policy: &str) -> Result<Value> {
    let mut value = parse(raw)?;
    if text(&value, "policy_id")? != policy {
        return Err(invalid("policy row identity mismatch"));
    }
    convert_identity(
        &mut value,
        map,
        workspace,
        policy,
        "authorization_revision",
        "authorization_key",
    )?;
    let _: server_api::launch_policy::WorkerLaunchPolicy =
        serde_json::from_value(value.clone()).map_err(invalid)?;
    Ok(value)
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 135 {
        return Err(invalid("requires schema 135"));
    }
    verify_schema_history(conn, 135)?;
    let foreign_keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
    let legacy_alter: bool = conn.query_row("PRAGMA legacy_alter_table", [], |r| r.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
    let result = (|| -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let mut map = Identities::new();
        let history = tx.prepare("SELECT h.workspace_id,h.policy_id,h.authorization_revision,h.operation_key,o.request_json FROM worker_launch_policy_history h JOIN worker_launch_policy_operations o ON o.workspace_id=h.workspace_id AND o.operation_key=h.operation_key")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (workspace, policy, ordinal, operation, raw) in history {
            let request = parse(&raw)?;
            if text(&request, "policy_id")? != policy
                || text(&request, "operation_key")? != operation
            {
                return Err(invalid("history request scope mismatch"));
            }
            match request
                .get("action")
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
            {
                Some("rename") => continue,
                Some("create" | "replace_authorization" | "revoke" | "delete") => {}
                _ => return Err(invalid("unknown saved policy action")),
            }
            if !(1..=9_007_199_254_740_991).contains(&ordinal) {
                return Err(invalid("invalid history identity"));
            }
            if map
                .insert(
                    (workspace, policy, ordinal),
                    format!("operation:{operation}"),
                )
                .is_some()
            {
                return Err(invalid(
                    "multiple authorization-changing operations share an identity",
                ));
            }
        }
        for table in [
            "worker_launch_policies",
            "worker_launch_policy_history",
            "worker_launch_policy_operations",
            "core_worker_execution_grants",
            "core_worker_grant_revocations",
        ] {
            tx.execute_batch(&format!("ALTER TABLE {table} RENAME TO saved_{table}_135;"))?;
        }
        tx.execute_batch("DROP INDEX worker_launch_policy_revision_history;")?;
        tx.execute_batch(crate::launch_policy::CURRENT_SCHEMA)?;
        let rows = tx.prepare("SELECT workspace_id,policy_id,authorization_revision,state,policy_json,last_operation_key FROM saved_worker_launch_policies_135")?
            .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (workspace, policy, ordinal, state, raw, last) in rows {
            let value = policy_json(&raw, &map, &workspace, &policy)?;
            let identity = key(&map, &workspace, &policy, &Value::from(ordinal))?;
            if text(&value, "authorization_key")? != identity
                || text(&value, "state")? != state
                || text(&value, "last_operation_key")? != last
            {
                return Err(invalid("policy projection mismatch"));
            }
            tx.execute(
                "INSERT INTO worker_launch_policies VALUES(?1,?2,?3,?4,?5,?6)",
                params![workspace, policy, identity, state, value.to_string(), last],
            )?;
        }
        let rows=tx.prepare("SELECT workspace_id,policy_id,operation_key,authorization_revision,policy_json FROM saved_worker_launch_policy_history_135")?
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,u64>(3)?,r.get::<_,String>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (workspace, policy, operation, ordinal, raw) in rows {
            let value = policy_json(&raw, &map, &workspace, &policy)?;
            let identity = key(&map, &workspace, &policy, &Value::from(ordinal))?;
            if text(&value, "authorization_key")? != identity
                || text(&value, "last_operation_key")? != operation
            {
                return Err(invalid("history projection mismatch"));
            }
            tx.execute(
                "INSERT INTO worker_launch_policy_history VALUES(?1,?2,?3,?4,?5)",
                params![workspace, policy, operation, identity, value.to_string()],
            )?;
        }
        let rows=tx.prepare("SELECT workspace_id,operation_key,actor_account_id,request_json,receipt_json FROM saved_worker_launch_policy_operations_135")?
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (workspace, operation, actor, raw, receipt) in rows {
            let mut request = parse(&raw)?;
            let policy = text(&request, "policy_id")?.to_owned();
            if let Some(expected) = request.get_mut("expected").filter(|v| !v.is_null()) {
                convert_identity(
                    expected,
                    &map,
                    &workspace,
                    &policy,
                    "authorization_revision",
                    "authorization_key",
                )?;
            }
            let typed: server_api::launch_policy::WorkerLaunchPolicyMutation =
                serde_json::from_value(request.clone()).map_err(invalid)?;
            typed.validate().map_err(invalid)?;
            let mut receipt = parse(&receipt)?;
            receipt["policy"] =
                policy_json(&receipt["policy"].to_string(), &map, &workspace, &policy)?;
            let saved_receipt: server_api::launch_policy::WorkerLaunchPolicyReceipt =
                serde_json::from_value(receipt.clone()).map_err(invalid)?;
            if typed.operation_key != operation
                || saved_receipt.operation_key != operation
                || saved_receipt.policy.last_operation_key != operation
            {
                return Err(invalid("saved operation receipt identity mismatch"));
            }
            tx.execute(
                "INSERT INTO worker_launch_policy_operations VALUES(?1,?2,?3,?4,?5)",
                params![
                    workspace,
                    operation,
                    actor,
                    request.to_string(),
                    receipt.to_string()
                ],
            )?;
        }
        let rows=tx.prepare("SELECT workspace_id,grant_id,creation_operation_id,runtime_id,worker_id,grant_json,policy_id,policy_revision,revoked FROM saved_core_worker_execution_grants_135")?
            .query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<u64>>(7)?,r.get::<_,bool>(8)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for (workspace, id, operation, runtime, worker, raw, policy, ordinal, revoked) in rows {
            let mut value = parse(&raw)?;
            let identity = match (&policy, ordinal) {
                (Some(policy), Some(ordinal)) => {
                    let source = value
                        .get_mut("source")
                        .ok_or_else(|| invalid("grant source missing"))?;
                    if text(source, "kind")? != "launch_policy"
                        || text(source, "policy_id")? != policy
                    {
                        return Err(invalid("grant policy source mismatch"));
                    }
                    convert_identity(
                        source,
                        &map,
                        &workspace,
                        policy,
                        "policy_revision",
                        "policy_authorization_key",
                    )?;
                    let identity = key(&map, &workspace, policy, &Value::from(ordinal))?;
                    if text(source, "policy_authorization_key")? != identity {
                        return Err(invalid("grant projection mismatch"));
                    }
                    Some(identity)
                }
                (None, None) => None,
                _ => return Err(invalid("partial grant policy identity")),
            };
            let grant: server_api::authorization::CoreWorkerExecutionGrant =
                serde_json::from_value(value.clone()).map_err(invalid)?;
            grant.validate().map_err(invalid)?;
            if grant.workspace_id != workspace
                || grant.grant_id != id
                || grant.creation_operation_id != operation
                || grant.runtime_id != runtime
                || grant.worker_id != worker
            {
                return Err(invalid("grant row identity mismatch"));
            }
            tx.execute(
                "INSERT INTO core_worker_execution_grants VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    workspace,
                    id,
                    operation,
                    runtime,
                    worker,
                    value.to_string(),
                    policy,
                    identity,
                    revoked
                ],
            )?;
        }
        tx.execute_batch("INSERT INTO core_worker_grant_revocations SELECT * FROM saved_core_worker_grant_revocations_135;")?;
        for table in [
            "core_worker_grant_revocations",
            "core_worker_execution_grants",
            "worker_launch_policy_operations",
            "worker_launch_policy_history",
            "worker_launch_policies",
        ] {
            tx.execute_batch(&format!("DROP TABLE saved_{table}_135;"))?;
        }
        let violations: i64 =
            tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })?;
        if violations != 0 {
            return Err(invalid("foreign key verification failed"));
        }
        tx.execute(
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(136,?1)",
            [MIGRATION_NAME],
        )?;
        tx.commit()?;
        Ok(())
    })();
    conn.pragma_update(None, "legacy_alter_table", legacy_alter)?;
    conn.pragma_update(None, "foreign_keys", foreign_keys)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn seed(conn: &Connection) {
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','t','t'); INSERT INTO workspaces(workspace_id,display_name,state,created_at,updated_at,owner_account_id) VALUES('w','W','active','t','t','owner');").unwrap();
        let assignment = json!({"grants":[],"exclusions":[],"revoked":false});
        let initial = json!({"policy_id":"coder","name":"Coder","authorization_revision":1,"state":"active","allowed_profiles":["builtin:coder"],"child_permissions":assignment,"reference_encoding":"feature_sets_v2","last_operation_key":"create"});
        let mut renamed = initial.clone();
        renamed["name"] = json!("Renamed");
        renamed["last_operation_key"] = json!("rename");
        let create = json!({"operation_key":"create","policy_id":"coder","expected":null,"action":{"kind":"create","name":"Coder","allowed_profiles":["builtin:coder"],"child_permissions":assignment}});
        let rename = json!({"operation_key":"rename","policy_id":"coder","expected":{"authorization_revision":1,"last_operation_key":"create"},"action":{"kind":"rename","name":"Renamed"}});
        conn.execute(
            "INSERT INTO worker_launch_policies VALUES('w','coder',1,'active',?1,'rename')",
            [renamed.to_string()],
        )
        .unwrap();
        for (operation, request, policy) in
            [("create", create, initial), ("rename", rename, renamed)]
        {
            conn.execute(
                "INSERT INTO worker_launch_policy_history VALUES('w','coder',?1,1,?2)",
                params![operation, policy.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO worker_launch_policy_operations VALUES('w',?1,'owner',?2,?3)",
                params![
                    operation,
                    request.to_string(),
                    json!({"policy":policy,"operation_key":operation}).to_string()
                ],
            )
            .unwrap();
        }
        let grant = json!({"grant_id":"g","workspace_id":"w","creation_operation_id":"spawn","runtime_id":"r","worker_id":"child","profile":"builtin:coder","profile_digest":"digest","assignment":assignment,"reference_encoding":"feature_sets_v2","issuance_ceiling":[],"source":{"kind":"launch_policy","policy_id":"coder","policy_revision":1,"parent":{"kind":"subject_body","subject_id":"s","source_change_id":"change","runtime_id":"r","worker_id":"body"}}});
        conn.execute("INSERT INTO core_worker_execution_grants VALUES('w','g','spawn','r','child',?1,'coder',1,0)",[grant.to_string()]).unwrap();
        conn.execute("INSERT INTO core_worker_launch_bindings VALUES('w','spawn','saved-caller-fingerprint','r','child','{}')",[]).unwrap();
    }
    #[test]
    fn retained_identity_migration_keeps_rename_grants_receipts_and_frozen_fingerprints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        super::super::tests::prepare_retained_schema(&path, 135);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        seed(&conn);
        migrate(&conn).unwrap();
        super::super::verify_schema_history(&conn, 136).unwrap();
        let policy = crate::launch_policy::policy(&conn, "w", "coder")
            .unwrap()
            .unwrap();
        assert_eq!(policy.authorization_key, "operation:create");
        assert_eq!(policy.last_operation_key, "rename");
        let grant = crate::launch_policy::core_grant(&conn, "w", "r", "child")
            .unwrap()
            .unwrap();
        crate::launch_policy::validate_policy_lineage(&conn, &grant).unwrap();
        let raw:String=conn.query_row("SELECT request_json FROM worker_launch_policy_operations WHERE operation_key='rename'",[],|r|r.get(0)).unwrap();
        let replay = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            crate::launch_policy::mutate(&conn, "w", "owner", &replay)
                .unwrap()
                .policy,
            policy
        );
        let fp: String = conn
            .query_row(
                "SELECT caller_fingerprint FROM core_worker_launch_bindings",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fp, "saved-caller-fingerprint");
        let replace = server_api::launch_policy::WorkerLaunchPolicyMutation {
            operation_key: "replace".into(),
            policy_id: "coder".into(),
            expected: Some(policy.observation()),
            action: server_api::launch_policy::WorkerLaunchPolicyAction::ReplaceAuthorization {
                allowed_profiles: policy.allowed_profiles.clone(),
                child_permissions: policy.child_permissions.clone(),
            },
        };
        let replaced = crate::launch_policy::mutate(&conn, "w", "owner", &replace)
            .unwrap()
            .policy;
        assert_ne!(replaced.authorization_key, policy.authorization_key);
        assert!(crate::launch_policy::validate_policy_lineage(&conn, &grant).is_err());
        drop(conn);
        let reopened = Connection::open(&path).unwrap();
        super::super::verify_schema_history(&reopened, 136).unwrap();
        assert!(crate::launch_policy::validate_policy_lineage(&reopened, &grant).is_err());
        assert_eq!(
            crate::launch_policy::mutate(&reopened, "w", "owner", &replay)
                .unwrap()
                .policy,
            policy
        );
    }
    #[test]
    fn migration_of_replaced_revoked_and_reverted_history_never_revives_original_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        super::super::tests::prepare_retained_schema(&path, 135);
        let conn = Connection::open(&path).unwrap();
        seed(&conn);
        let raw: String = conn
            .query_row("SELECT policy_json FROM worker_launch_policies", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut previous = parse(&raw).unwrap();
        for (ordinal, operation, state, action) in [
            (
                2,
                "replace",
                "active",
                json!({"kind":"replace_authorization","allowed_profiles":previous["allowed_profiles"],"child_permissions":previous["child_permissions"]}),
            ),
            (3, "revoke", "revoked", json!({"kind":"revoke"})),
            (
                4,
                "restore-content",
                "active",
                json!({"kind":"replace_authorization","allowed_profiles":previous["allowed_profiles"],"child_permissions":previous["child_permissions"]}),
            ),
        ] {
            let request = json!({"operation_key":operation,"policy_id":"coder","expected":{"authorization_revision":previous["authorization_revision"],"last_operation_key":previous["last_operation_key"]},"action":action});
            previous["authorization_revision"] = json!(ordinal);
            previous["last_operation_key"] = json!(operation);
            previous["state"] = json!(state);
            conn.execute(
                "INSERT INTO worker_launch_policy_history VALUES('w','coder',?1,?2,?3)",
                params![operation, ordinal, previous.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO worker_launch_policy_operations VALUES('w',?1,'owner',?2,?3)",
                params![
                    operation,
                    request.to_string(),
                    json!({"operation_key":operation,"policy":previous}).to_string()
                ],
            )
            .unwrap();
            conn.execute("UPDATE worker_launch_policies SET authorization_revision=?1,state=?2,policy_json=?3,last_operation_key=?4",params![ordinal,state,previous.to_string(),operation]).unwrap();
        }
        migrate(&conn).unwrap();
        let current = crate::launch_policy::policy(&conn, "w", "coder")
            .unwrap()
            .unwrap();
        assert_eq!(current.authorization_key, "operation:restore-content");
        let grant = crate::launch_policy::core_grant(&conn, "w", "r", "child")
            .unwrap()
            .unwrap();
        assert!(crate::launch_policy::validate_policy_lineage(&conn, &grant).is_err());
        let identities: Vec<String> = conn.prepare("SELECT DISTINCT authorization_key FROM worker_launch_policy_history ORDER BY authorization_key").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(
            identities,
            [
                "operation:create",
                "operation:replace",
                "operation:restore-content",
                "operation:revoke"
            ]
        );
    }

    #[test]
    fn identity_migration_receipt_failure_rolls_back_then_retries_without_partial_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        super::super::tests::prepare_retained_schema(&path, 135);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        seed(&conn);
        conn.execute_batch("CREATE TEMP TRIGGER reject_policy_identity_receipt BEFORE INSERT ON main.__yoi_schema_migrations WHEN NEW.version=136 BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(migrate(&conn).is_err());
        assert_eq!(current_schema_version(&conn).unwrap(), 135);
        assert!(column_exists(&conn, "worker_launch_policies", "authorization_revision").unwrap());
        assert!(!column_exists(&conn, "worker_launch_policies", "authorization_key").unwrap());
        assert!(
            conn.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
                .unwrap()
        );
        conn.execute_batch("DROP TRIGGER reject_policy_identity_receipt")
            .unwrap();
        migrate(&conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 136);
    }
}
