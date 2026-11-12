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
        conn.execute_batch(CURRENT_SCHEMA).unwrap();
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
            policy_authorization_key: p.authorization_key.clone(),
            parent: CoreWorkerGrantParent::SubjectBody {
                subject_id: "s".into(),
                source_change_id: "subject-v1".into(),
                runtime_id: "r".into(),
                worker_id: "body".into(),
            },
        },
    }
}

#[test]
fn assignment_delegation_keeps_finite_empty_bound_and_rechecks_revoked_parent_after_reopen() {
    let mut f = Fixture::new();
    let p = f.change(&create()).policy;
    let mut root = grant(&p, "account-root");
    root.source = CoreWorkerGrantSource::Account {
        account_id: "owner".into(),
    };
    root.issuance_ceiling = None;
    f.bind(&root);
    let mut child = grant(&p, "assignment-child");
    child.source = CoreWorkerGrantSource::Delegation {
        parent: CoreWorkerGrantParent::WorkerGrant {
            grant_id: root.grant_id.clone(),
            runtime_id: root.runtime_id.clone(),
            worker_id: root.worker_id.clone(),
        },
    };
    child.assignment = Default::default();
    child.issuance_ceiling = None;
    assert!(bind_core_grant(&f.conn, &child).is_err());
    child.issuance_ceiling = Some(vec![]);
    f.bind(&child);
    f.reopen();
    assert!(validate_policy_lineage(&f.conn, &child).unwrap().is_empty());
    assert_eq!(
        core_grant(&f.conn, "w", "r", "assignment-child").unwrap(),
        Some(child.clone())
    );
    assert!(
        bounded_current_keys(&child, [PermissionKey::parse("worker.create").unwrap()]).is_empty()
    );
    revoke_core_grant(&f.conn, "w", "owner", &root.grant_id, "revoke-root").unwrap();
    f.reopen();
    assert!(validate_policy_lineage(&f.conn, &child).is_err());
    // Immutable exact replay is not authority to launch or act again.
    assert_eq!(bind_core_grant(&f.conn, &child).unwrap(), child);
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
        renamed.policy.authorization_key,
        initial.policy.authorization_key
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
    assert!(replaced.authorization_key != initial.authorization_key);
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
            &active.authorization_key,
            "project:coder"
        )
        .is_err()
    );
    assert!(
        require_current_policy(
            &f.conn,
            "other",
            "coder",
            &active.authorization_key,
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
