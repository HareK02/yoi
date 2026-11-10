//! Component tests of the production SQL authority using temporary SQLite files.
//! The only replaced boundary is the unrelated Workspace catalog schema. These
//! tests do not assert HTTP owner authentication, Host issuance or Runtime dispatch.

use super::*;
use feature_permissions::{PermissionAssignment, PermissionReference, PermissionSetId};

struct Fixture {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    conn: Connection,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE workspaces (workspace_id TEXT PRIMARY KEY, owner_account_id TEXT NOT NULL, state TEXT NOT NULL); INSERT INTO workspaces VALUES('w','owner','active'),('other','owner','active');").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        Self {
            _dir: dir,
            path,
            conn,
        }
    }
    fn change(&mut self, request: &WorkerLaunchPolicyMutation) -> WorkerLaunchPolicyReceipt {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let receipt = mutate(&tx, "w", "owner", request).unwrap();
        tx.commit().unwrap();
        receipt
    }
    fn bind(&mut self, grant: &CoreWorkerExecutionGrant) {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        bind_core_grant(&tx, grant).unwrap();
        tx.commit().unwrap();
    }
    fn reopen(&mut self) {
        self.conn = Connection::open(&self.path).unwrap();
        self.conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    }
}
fn assignment() -> PermissionAssignment {
    PermissionAssignment {
        grants: vec![PermissionReference::Set(
            PermissionSetId::parse("child").unwrap(),
        )],
        exclusions: vec![],
        revoked: false,
    }
}
fn create() -> WorkerLaunchPolicyMutation {
    WorkerLaunchPolicyMutation {
        operation_key: "create".into(),
        policy_id: "coder".into(),
        expected: None,
        action: WorkerLaunchPolicyAction::Create {
            name: "Coder".into(),
            allowed_profiles: vec!["builtin:coder".into()],
            child_permissions: assignment(),
        },
    }
}
fn change(
    p: &WorkerLaunchPolicy,
    key: &str,
    action: WorkerLaunchPolicyAction,
) -> WorkerLaunchPolicyMutation {
    WorkerLaunchPolicyMutation {
        operation_key: key.into(),
        policy_id: p.policy_id.clone(),
        expected: Some(p.observation()),
        action,
    }
}
fn grant(p: &WorkerLaunchPolicy, id: &str) -> CoreWorkerExecutionGrant {
    CoreWorkerExecutionGrant {
        grant_id: id.into(),
        workspace_id: "w".into(),
        creation_operation_id: format!("creation-{id}"),
        runtime_id: "r".into(),
        worker_id: id.into(),
        profile: "builtin:coder".into(),
        profile_digest: "bound-digest".into(),
        assignment: p.effective_child_permissions(),
        reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
        issuance_ceiling: Some(vec![PermissionKey::parse("worker.create").unwrap()]),
        source: CoreWorkerGrantSource::LaunchPolicy {
            policy_id: p.policy_id.clone(),
            policy_revision: p.authorization_revision,
            parent: CoreWorkerGrantParent::SubjectBody {
                subject_id: "s".into(),
                source_change_id: "subject-v1".into(),
                runtime_id: "r".into(),
                worker_id: "body".into(),
            },
        },
    }
}

// Historical rows are seeded directly, not admitted as new current-catalog input.
fn seed_legacy_rows(
    f: &mut Fixture,
) -> (
    WorkerLaunchPolicyMutation,
    WorkerLaunchPolicyReceipt,
    CoreWorkerExecutionGrant,
) {
    let policy_raw = concat!(
        r#"{"policy_id":"coder","name":"Coder","authorization_revision":1,"state":"active","allowed_profiles":["builtin:coder"],"child_permissions":{"grants":["#,
        r#"{"kind":"set","value":"subject-default"}],"exclusions":[{"kind":"namespace","value":{"namespace":"subject","keys":["subject.delegate"]}}],"revoked":false},"#,
        r#""last_operation_key":"create"}"#,
    );
    let p: WorkerLaunchPolicy = decode(policy_raw).unwrap();
    let mut request = create();
    if let WorkerLaunchPolicyAction::Create {
        child_permissions, ..
    } = &mut request.action
    {
        *child_permissions = p.child_permissions.clone();
    }
    let receipt_raw = format!(r#"{{"policy":{policy_raw},"operation_key":"create"}}"#);
    let receipt: WorkerLaunchPolicyReceipt = decode(&receipt_raw).unwrap();
    let mut child = grant(&p, "child");
    child.assignment = p.child_permissions.clone();
    child.reference_encoding = PermissionReferenceEncoding::LegacySubjectV1;
    f.conn
        .execute(
            "INSERT INTO worker_launch_policies VALUES('w','coder',1,'active',?1,'create')",
            [policy_raw],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO worker_launch_policy_history VALUES('w','coder','create',1,?1)",
            [policy_raw],
        )
        .unwrap();
    f.conn
        .execute(
            "INSERT INTO worker_launch_policy_operations VALUES('w','create','owner',?1,?2)",
            params![encode(&request).unwrap(), receipt_raw],
        )
        .unwrap();
    f.conn.execute(
        "INSERT INTO core_worker_execution_grants(workspace_id,grant_id,creation_operation_id,runtime_id,worker_id,grant_json,policy_id,policy_revision) VALUES('w','child','creation-child','r','child',?1,'coder',1)",
        [encode(&child).unwrap()],
    ).unwrap();
    (request, receipt, child)
}

fn legacy_row_bytes(conn: &Connection) -> [String; 5] {
    conn.query_row(
        "SELECT p.policy_json,g.grant_json,o.request_json,o.receipt_json,h.policy_json FROM worker_launch_policies p JOIN core_worker_execution_grants g ON g.workspace_id=p.workspace_id JOIN worker_launch_policy_operations o ON o.workspace_id=p.workspace_id JOIN worker_launch_policy_history h ON h.workspace_id=p.workspace_id WHERE p.workspace_id='w' AND p.policy_id='coder' AND g.grant_id='child' AND o.operation_key='create' AND h.operation_key='create'",
        [],
        |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?]),
    ).unwrap()
}

#[test]
fn legacy_sqlite_reads_and_exact_replay_preserve_raw_rows_without_new_grants() {
    let mut f = Fixture::new();
    let (request, receipt, child) = seed_legacy_rows(&mut f);
    let original = legacy_row_bytes(&f.conn);
    assert!(
        original
            .iter()
            .all(|raw| !raw.contains("reference_encoding"))
    );
    assert_eq!(encode(&receipt.policy).unwrap(), original[0]);
    assert_eq!(encode(&child).unwrap(), original[1]);
    assert_eq!(encode(&receipt).unwrap(), original[3]);
    f.reopen();
    let current = policy(&f.conn, "w", "coder").unwrap().unwrap();
    assert_eq!(
        current.reference_encoding,
        PermissionReferenceEncoding::FeatureSetsV2
    );
    assert_eq!(
        current.child_permissions,
        receipt.policy.effective_child_permissions()
    );
    assert_eq!(current.observation(), receipt.policy.observation());
    assert_eq!(current.state, receipt.policy.state);
    assert_eq!(current.allowed_profiles, receipt.policy.allowed_profiles);
    assert_eq!(policies(&f.conn, "w").unwrap(), vec![current.clone()]);
    let saved = core_grant(&f.conn, "w", "r", "child").unwrap().unwrap();
    assert_eq!(saved, child);
    assert_eq!(saved.effective_assignment(), current.child_permissions);
    assert_eq!(saved.issuance_ceiling, child.issuance_ceiling);
    assert_eq!(f.change(&request), receipt);
    assert_eq!(bind_core_grant(&f.conn, &saved).unwrap(), child);
    let mut projected = saved.clone();
    projected.assignment = saved.effective_assignment();
    projected.reference_encoding = PermissionReferenceEncoding::FeatureSetsV2;
    assert!(bind_core_grant(&f.conn, &projected).is_err());
    let mut changed_request = request.clone();
    if let WorkerLaunchPolicyAction::Create {
        child_permissions, ..
    } = &mut changed_request.action
    {
        *child_permissions = current.child_permissions;
    }
    assert!(mutate(&f.conn, "w", "owner", &changed_request).is_err());
    f.reopen();
    assert_eq!(legacy_row_bytes(&f.conn), original);
    let count: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM core_worker_execution_grants",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn legacy_grant_matches_v2_current_policy_at_same_revision_but_not_replacement_or_revocation() {
    let mut f = Fixture::new();
    let (request, receipt, child) = seed_legacy_rows(&mut f);
    let original = legacy_row_bytes(&f.conn);
    let renamed = f
        .change(&change(
            &receipt.policy,
            "rename-legacy",
            WorkerLaunchPolicyAction::Rename {
                name: "Current".into(),
            },
        ))
        .policy;
    assert_eq!(
        renamed.reference_encoding,
        PermissionReferenceEncoding::FeatureSetsV2
    );
    assert_eq!(
        renamed.authorization_revision,
        receipt.policy.authorization_revision
    );
    assert_eq!(renamed.child_permissions, child.effective_assignment());
    f.reopen();
    assert_eq!(validate_policy_lineage(&f.conn, &child).unwrap().len(), 1);
    assert_eq!(
        core_grant(&f.conn, "w", "r", "child").unwrap().unwrap(),
        child
    );
    let stop = PermissionKey::parse("worker.stop").unwrap();
    let create = PermissionKey::parse("worker.create").unwrap();
    assert_eq!(
        bounded_current_keys(&child, [create.clone(), stop]),
        vec![create]
    );
    let replaced = f
        .change(&change(
            &renamed,
            "replace-legacy",
            WorkerLaunchPolicyAction::ReplaceAuthorization {
                allowed_profiles: renamed.allowed_profiles.clone(),
                child_permissions: renamed.child_permissions.clone(),
            },
        ))
        .policy;
    assert!(validate_policy_lineage(&f.conn, &child).is_err());
    assert_eq!(bind_core_grant(&f.conn, &child).unwrap(), child);
    let mut new_child = grant(&replaced, "new-child");
    new_child.issuance_ceiling = Some(vec![]);
    f.bind(&new_child);
    validate_policy_lineage(&f.conn, &new_child).unwrap();
    assert!(
        bounded_current_keys(&new_child, [PermissionKey::parse("worker.create").unwrap()])
            .is_empty()
    );
    f.change(&change(
        &replaced,
        "revoke-legacy",
        WorkerLaunchPolicyAction::Revoke,
    ));
    f.reopen();
    assert!(validate_policy_lineage(&f.conn, &new_child).is_err());
    assert_eq!(bind_core_grant(&f.conn, &new_child).unwrap(), new_child);
    assert_eq!(
        core_grant(&f.conn, "w", "r", "new-child")
            .unwrap()
            .unwrap()
            .issuance_ceiling,
        Some(vec![])
    );
    assert_eq!(
        core_grant(&f.conn, "w", "r", "child")
            .unwrap()
            .unwrap()
            .issuance_ceiling,
        child.issuance_ceiling
    );
    assert_eq!(f.change(&request), receipt);
    assert_eq!(legacy_row_bytes(&f.conn)[1..], original[1..]);
}

#[test]
fn legacy_current_policy_reads_preserve_state_profiles_revision_and_last_operation() {
    let mut f = Fixture::new();
    let (_, receipt, _) = seed_legacy_rows(&mut f);
    for (state, state_name) in [
        (WorkerLaunchPolicyState::Active, "active"),
        (WorkerLaunchPolicyState::Revoked, "revoked"),
        (WorkerLaunchPolicyState::Deleted, "deleted"),
    ] {
        let mut saved = receipt.policy.clone();
        saved.state = state;
        saved.authorization_revision = 7;
        saved.last_operation_key = "retained-operation".into();
        saved.allowed_profiles.push("project:reviewer".into());
        saved.child_permissions.revoked = true;
        let raw = encode(&saved).unwrap();
        f.conn.execute(
            "UPDATE worker_launch_policies SET policy_json=?1,state=?2,authorization_revision=7,last_operation_key='retained-operation' WHERE workspace_id='w' AND policy_id='coder'",
            params![raw, state_name],
        ).unwrap();
        f.reopen();
        let current = policy(&f.conn, "w", "coder").unwrap().unwrap();
        assert_eq!(current.observation(), saved.observation());
        assert_eq!(current.allowed_profiles, saved.allowed_profiles);
        assert_eq!(current.state, state);
        assert!(current.child_permissions.revoked);
        assert_eq!(
            current.child_permissions,
            saved.effective_child_permissions()
        );
        assert_eq!(policies(&f.conn, "w").unwrap(), vec![current]);
        assert_eq!(legacy_row_bytes(&f.conn)[0], raw);
    }
}

#[test]
fn legacy_human_grant_projection_preserves_dynamic_and_explicit_empty_sqlite_ceilings() {
    let mut f = Fixture::new();
    let (_, _, template) = seed_legacy_rows(&mut f);
    for (id, ceiling) in [("dynamic", None), ("empty", Some(vec![]))] {
        let mut human = template.clone();
        human.grant_id = id.into();
        human.worker_id = id.into();
        human.creation_operation_id = format!("creation-{id}");
        human.source = CoreWorkerGrantSource::Human {
            account_id: "owner".into(),
        };
        human.issuance_ceiling = ceiling.clone();
        f.bind(&human);
        f.reopen();
        let saved = core_grant(&f.conn, "w", "r", id).unwrap().unwrap();
        assert_eq!(
            saved.effective_assignment(),
            template.effective_assignment()
        );
        assert_eq!(saved.issuance_ceiling, ceiling);
        assert_eq!(encode(&saved).unwrap(), encode(&human).unwrap());
        assert_eq!(bind_core_grant(&f.conn, &saved).unwrap(), human);
    }
}

#[test]
fn owner_cas_replay_and_rename_history_survive_reopen_without_reauthorization() {
    let mut f = Fixture::new();
    assert!(mutate(&f.conn, "w", "not-owner", &create()).is_err());
    assert!(policies(&f.conn, "w").unwrap().is_empty());
    let initial = f.change(&create());
    assert_eq!(
        initial.policy.reference_encoding,
        PermissionReferenceEncoding::FeatureSetsV2
    );
    let rename = change(
        &initial.policy,
        "rename",
        WorkerLaunchPolicyAction::Rename {
            name: "Renamed".into(),
        },
    );
    let renamed = f.change(&rename);
    assert_eq!(
        renamed.policy.authorization_revision,
        initial.policy.authorization_revision
    );
    assert_eq!(
        renamed.policy.usage_key().unwrap(),
        initial.policy.usage_key().unwrap()
    );
    assert!(
        mutate(
            &f.conn,
            "w",
            "owner",
            &change(&initial.policy, "stale", WorkerLaunchPolicyAction::Revoke)
        )
        .is_err()
    );
    f.reopen();
    assert_eq!(f.change(&create()), initial);
    assert_eq!(f.change(&rename), renamed);
    let mut different = rename.clone();
    different.action = WorkerLaunchPolicyAction::Rename {
        name: "Other".into(),
    };
    assert!(mutate(&f.conn, "w", "owner", &different).is_err());
    assert_eq!(
        policy(&f.conn, "w", "coder").unwrap().unwrap(),
        renamed.policy
    );
    assert!(policy(&f.conn, "other", "coder").unwrap().is_none());
    let history: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM worker_launch_policy_history",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 2);
    f.conn
        .execute(
            "UPDATE workspaces SET owner_account_id='replacement' WHERE workspace_id='w'",
            [],
        )
        .unwrap();
    assert!(mutate(&f.conn, "w", "owner", &rename).is_err());
    assert!(mutate(&f.conn, "w", "replacement", &rename).is_err());
}

#[test]
fn authorization_replacement_revoke_delete_and_revert_never_revive_old_grants() {
    let mut f = Fixture::new();
    let initial = f.change(&create()).policy;
    let child = grant(&initial, "child");
    f.bind(&child);
    assert_eq!(validate_policy_lineage(&f.conn, &child).unwrap().len(), 1);
    let renamed = f
        .change(&change(
            &initial,
            "rename",
            WorkerLaunchPolicyAction::Rename {
                name: "Display only".into(),
            },
        ))
        .policy;
    validate_policy_lineage(&f.conn, &child).unwrap();
    let replaced = f
        .change(&change(
            &renamed,
            "replace",
            WorkerLaunchPolicyAction::ReplaceAuthorization {
                allowed_profiles: initial.allowed_profiles.clone(),
                child_permissions: initial.child_permissions.clone(),
            },
        ))
        .policy;
    assert!(replaced.authorization_revision > initial.authorization_revision);
    assert!(validate_policy_lineage(&f.conn, &child).is_err());
    // Exact binding replay is historical, never permission to dispatch.
    assert_eq!(bind_core_grant(&f.conn, &child).unwrap(), child);
    assert!(validate_policy_lineage(&f.conn, &child).is_err());
    let new_child = grant(&replaced, "new-child");
    f.bind(&new_child);
    let revoked = f
        .change(&change(
            &replaced,
            "revoke",
            WorkerLaunchPolicyAction::Revoke,
        ))
        .policy;
    assert!(validate_policy_lineage(&f.conn, &new_child).is_err());
    let active = f
        .change(&change(
            &revoked,
            "revert",
            WorkerLaunchPolicyAction::ReplaceAuthorization {
                allowed_profiles: initial.allowed_profiles.clone(),
                child_permissions: initial.child_permissions.clone(),
            },
        ))
        .policy;
    f.reopen();
    assert!(validate_policy_lineage(&f.conn, &child).is_err());
    assert!(validate_policy_lineage(&f.conn, &new_child).is_err());
    assert!(
        require_current_policy(
            &f.conn,
            "w",
            "coder",
            active.authorization_revision,
            "project:coder"
        )
        .is_err()
    );
    assert!(
        require_current_policy(
            &f.conn,
            "other",
            "coder",
            active.authorization_revision,
            "builtin:coder"
        )
        .is_err()
    );
    let deleted = f
        .change(&change(&active, "delete", WorkerLaunchPolicyAction::Delete))
        .policy;
    assert!(usage_declarations(&[deleted.clone()]).unwrap().is_empty());
    let mut recreate = create();
    recreate.operation_key = "recreate".into();
    assert!(mutate(&f.conn, "w", "owner", &recreate).is_err());
    assert!(
        mutate(
            &f.conn,
            "w",
            "owner",
            &change(
                &deleted,
                "undelete",
                WorkerLaunchPolicyAction::ReplaceAuthorization {
                    allowed_profiles: initial.allowed_profiles,
                    child_permissions: initial.child_permissions
                }
            )
        )
        .is_err()
    );
    f.reopen();
    assert_eq!(
        policy(&f.conn, "w", "coder").unwrap().unwrap().state,
        WorkerLaunchPolicyState::Deleted
    );
}

#[test]
fn immutable_grant_binding_and_parent_revocation_survive_reopen() {
    let mut f = Fixture::new();
    let p = f.change(&create()).policy;
    let parent = grant(&p, "parent");
    f.bind(&parent);
    let mut child = grant(&p, "child");
    if let CoreWorkerGrantSource::LaunchPolicy { parent: p, .. } = &mut child.source {
        *p = CoreWorkerGrantParent::WorkerGrant {
            grant_id: parent.grant_id.clone(),
            runtime_id: parent.runtime_id.clone(),
            worker_id: parent.worker_id.clone(),
        };
    }
    f.bind(&child);
    let mut conflicting = child.clone();
    conflicting.profile_digest = "latest-not-original".into();
    assert!(bind_core_grant(&f.conn, &conflicting).is_err());
    conflicting = child.clone();
    conflicting
        .issuance_ceiling
        .as_mut()
        .unwrap()
        .push(PermissionKey::parse("worker.stop").unwrap());
    assert!(bind_core_grant(&f.conn, &conflicting).is_err());
    f.reopen();
    assert_eq!(
        core_grant(&f.conn, "w", "r", "child").unwrap().unwrap(),
        child
    );
    validate_policy_lineage(&f.conn, &child).unwrap();
    let tx = f
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    revoke_core_grant(&tx, "w", "owner", &parent.grant_id, "revoke-parent").unwrap();
    tx.commit().unwrap();
    f.reopen();
    assert!(validate_policy_lineage(&f.conn, &child).is_err());
    revoke_core_grant(&f.conn, "w", "owner", &parent.grant_id, "revoke-parent").unwrap();
    assert!(revoke_core_grant(&f.conn, "w", "owner", &child.grant_id, "revoke-parent").is_err());
    assert!(
        core_grant(&f.conn, "other", "r", "child")
            .unwrap()
            .is_none()
    );
}

#[test]
fn stale_independent_writer_and_failed_receipt_leave_no_partial_policy() {
    let mut f = Fixture::new();
    let initial = f.change(&create()).policy;
    let mut other = Connection::open(&f.path).unwrap();
    let observation = policy(&other, "w", "coder").unwrap().unwrap();
    let renamed = f
        .change(&change(
            &initial,
            "first-writer",
            WorkerLaunchPolicyAction::Rename { name: "One".into() },
        ))
        .policy;
    let tx = other
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(
        mutate(
            &tx,
            "w",
            "owner",
            &change(
                &observation,
                "second-writer",
                WorkerLaunchPolicyAction::Revoke
            )
        )
        .is_err()
    );
    tx.rollback().unwrap();
    f.conn.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON worker_launch_policy_operations BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
    let tx = f
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    assert!(
        mutate(
            &tx,
            "w",
            "owner",
            &change(&renamed, "failed", WorkerLaunchPolicyAction::Revoke)
        )
        .is_err()
    );
    tx.rollback().unwrap();
    f.reopen();
    assert_eq!(policy(&f.conn, "w", "coder").unwrap().unwrap(), renamed);
    let count: i64 = f
        .conn
        .query_row(
            "SELECT count(*) FROM worker_launch_policy_history WHERE operation_key='failed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn dynamic_set_growth_is_bounded_for_existing_children_and_shrink_disable_exclude_revoke_apply() {
    use feature_permissions::*;
    let mut f = Fixture::new();
    let p = f.change(&create()).policy;
    let child = grant(&p, "child");
    f.bind(&child);
    f.reopen();
    let child = core_grant(&f.conn, "w", "r", "child").unwrap().unwrap();
    let mut host = PermissionHost::new(WorkspaceId::parse("w").unwrap());
    let owner = host
        .register_feature(FeatureIdentity::parse("worker-feature").unwrap())
        .unwrap();
    let namespace = host
        .bind_namespace(&owner, NamespaceId::parse("worker").unwrap())
        .unwrap();
    let create_key = PermissionKey::parse("worker.create").unwrap();
    let stop_key = PermissionKey::parse("worker.stop").unwrap();
    let declaration = |keys: Vec<PermissionKey>| NamespaceDeclaration {
        permissions: keys
            .iter()
            .map(|key| PermissionDefinition {
                key: key.clone(),
                description: "operation".into(),
                children: vec![],
            })
            .collect(),
        sets: vec![SetContribution {
            set: PermissionSetId::parse("child").unwrap(),
            children: vec![PermissionReference::Namespace(NamespacePermissions {
                namespace: NamespaceId::parse("worker").unwrap(),
                keys,
            })],
        }],
    };
    host.declare(&namespace, declaration(vec![create_key.clone()]))
        .unwrap();
    let current = |host: &PermissionHost, a: &PermissionAssignment| {
        host.resolve(Some(a))
            .projection()
            .allowed
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        bounded_current_keys(&child, current(&host, &child.assignment)),
        vec![create_key.clone()]
    );
    host.declare(
        &namespace,
        declaration(vec![create_key.clone(), stop_key.clone()]),
    )
    .unwrap();
    assert_eq!(current(&host, &child.assignment).len(), 2);
    assert_eq!(
        bounded_current_keys(&child, current(&host, &child.assignment)),
        vec![create_key.clone()]
    );
    let mut new_child = grant(&p, "new-child");
    new_child.issuance_ceiling = Some(current(&host, &new_child.assignment));
    f.bind(&new_child);
    assert_eq!(
        bounded_current_keys(&new_child, current(&host, &new_child.assignment)).len(),
        2
    );
    host.declare(&namespace, declaration(vec![stop_key.clone()]))
        .unwrap();
    assert!(bounded_current_keys(&child, current(&host, &child.assignment)).is_empty());
    let mut excluded = child.assignment.clone();
    excluded.exclusions = vec![PermissionReference::Namespace(NamespacePermissions {
        namespace: NamespaceId::parse("worker").unwrap(),
        keys: vec![stop_key],
    })];
    assert!(bounded_current_keys(&new_child, current(&host, &excluded)).is_empty());
    excluded = child.assignment.clone();
    excluded.revoked = true;
    assert!(bounded_current_keys(&child, current(&host, &excluded)).is_empty());
    // Disabling/removing the Feature is represented by a current host without
    // its namespace. Existing references do not authorize absent declarations.
    let disabled = PermissionHost::new(WorkspaceId::parse("w").unwrap());
    assert!(bounded_current_keys(&child, current(&disabled, &child.assignment)).is_empty());
}
