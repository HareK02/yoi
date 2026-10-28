//! Real file-backed SQLite, production catalog operations and a checked-in PDK
//! Component. Execution/Host side effects are intentionally outside this boundary.
use super::*;
use crate::store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};
use rusqlite::{Connection, TransactionBehavior, params};
use server_api::*;
use std::fs;
use tempfile::{TempDir, tempdir};

struct Fixture {
    dir: TempDir,
    store: SqliteWorkspaceStore,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempdir().unwrap();
        let store = SqliteWorkspaceStore::open_for_test(dir.path().join("server.db")).unwrap();
        for workspace in ["a", "b"] {
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: workspace.into(),
                    owner_account_id: "owner-account".into(),
                    display_name: workspace.into(),
                    state: "active".into(),
                    created_at: "2026-10-28T00:00:00Z".into(),
                    updated_at: "2026-10-28T00:00:00Z".into(),
                })
                .await
                .unwrap();
        }
        Self { dir, store }
    }
    fn install(&self, alias: &str) -> PluginOperationResponse {
        let upload = self
            .store
            .plugin_upload(
                &self.dir.path().join("blobs"),
                "a",
                "actor",
                &package("0.1.0"),
            )
            .unwrap();
        self.store
            .plugin_install(
                "a",
                "actor",
                &PluginInstallRequest {
                    upload_id: upload.upload_id,
                    alias: alias.into(),
                    operation_key: alias.into(),
                },
            )
            .unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        self.store
            .with_conn(|conn| {
                Ok(conn.query_row(
                    &format!(
                        "SELECT count(*) FROM {table}{}",
                        if table == "plugin_references" {
                            " WHERE kind != 'config'"
                        } else {
                            ""
                        }
                    ),
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }
    fn kind(&self) -> String {
        self.store
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT kind FROM plugin_references WHERE kind != 'config' LIMIT 1",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }
    fn transition(&self, id: &str, from: PlanPinState, to: PlanPinState) -> StoredPluginPlan {
        self.store
            .plugin_plan_transition("a", id, from, to)
            .unwrap()
    }
}
fn package(version: &str) -> Vec<u8> {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("plugin.toml"),
        include_str!("../../../../resources/plugin/templates/rust-component-tool/plugin.toml")
            .replace("version = \"0.1.0\"", &format!("version = \"{version}\"")),
    )
    .unwrap();
    fs::write(
        dir.path().join("plugin.component.wasm"),
        include_bytes!("../../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
    )
    .unwrap();
    let package = plugin::read_plugin_directory(
        dir.path(),
        plugin::PluginSourceKind::Explicit,
        &plugin::PluginPackageLimits::default(),
    )
    .unwrap();
    plugin::pack_plugin_package(&package, &plugin::PluginPackageLimits::default())
        .unwrap()
        .bytes
}
fn digest(seed: &str) -> String {
    hash_bytes(seed.as_bytes())
}
fn projection() -> PluginPinProjection {
    PluginPinProjection {
        config: serde_json::json!({}),
        surfaces: vec![PluginSurface::Tool],
        tools: vec!["example_echo".into()],
        grants: vec![],
        bindings: vec![],
    }
}
fn pin(installed: &PluginOperationResponse) -> WorkerPluginPin {
    WorkerPluginPin::from_artifact(
        installed.installation.installation_id.clone(),
        installed.operation_id.clone(),
        &installed.installation.artifacts[0],
        projection(),
    )
    .unwrap()
}
fn input(plugins: Vec<WorkerPluginPin>) -> WorkerPluginPlanInput {
    WorkerPluginPlanInput {
        workspace_id: "a".into(),
        config: ConfigProvenance {
            content_digest: digest("config"),
            toolchain_fingerprint: digest("toolchain"),
        },
        profile: ProfileProvenance {
            identity: "profile:coder".into(),
            projection_digest: digest("profile"),
        },
        execution: HostExecutionIdentity {
            runtime_id: "runtime".into(),
            worker_id: "worker".into(),
            principal_id: "worker-principal".into(),
            delegation_id: Some("delegation".into()),
        },
        plugins,
    }
}
fn plan(plugins: Vec<WorkerPluginPin>) -> WorkerPluginExecutionPlan {
    WorkerPluginExecutionPlan::canonicalize(input(plugins)).unwrap()
}
fn error<T: std::fmt::Debug>(result: Result<T>, expected: PluginCatalogError) {
    assert!(
        matches!(result,Err(Error::PluginCatalog(actual)) if actual == expected),
        "expected {expected:?}, got {result:?}"
    );
}
fn uninstall(
    f: &Fixture,
    installation: &PluginOperationResponse,
    key: &str,
) -> Result<PluginOperationResponse> {
    f.store.plugin_uninstall(
        "a",
        "actor",
        &installation.installation.installation_id,
        &PluginUninstallRequest {
            expected_last_operation: installation.operation_id.clone(),
            operation_key: key.into(),
        },
    )
}

#[tokio::test]
async fn canonical_order_and_fingerprint_bind_all_provenance_and_both_digests() {
    let f = Fixture::new().await;
    let a = f.install("one");
    let b = f.install("two");
    let p = plan(vec![pin(&a), pin(&b)]);
    assert_eq!(p, plan(vec![pin(&b), pin(&a)]));
    assert_eq!(
        p.fingerprint().unwrap(),
        plan(vec![pin(&b), pin(&a)]).fingerprint().unwrap()
    );
    let mut changes = Vec::new();
    let base = input(vec![pin(&a), pin(&b)]);
    let mut changed = base.clone();
    changed.config.content_digest = digest("new config");
    changes.push(changed);
    let mut changed = base.clone();
    changed.config.toolchain_fingerprint = digest("new toolchain");
    changes.push(changed);
    let mut changed = base.clone();
    changed.profile.projection_digest = digest("new profile");
    changes.push(changed);
    let mut changed = base.clone();
    changed.execution.delegation_id = Some("different".into());
    changes.push(changed);
    let mut changed = base.clone();
    changed.plugins[0].package_digest = digest("new package");
    changes.push(changed);
    let mut changed = base.clone();
    changed.plugins[0].artifact_digest = digest("new artifact");
    changes.push(changed);
    let mut changed = base;
    changed.plugins[0].expected_last_operation = "new observation".into();
    changes.push(changed);
    for changed in changes {
        assert_ne!(
            p.fingerprint().unwrap(),
            WorkerPluginExecutionPlan::canonicalize(changed)
                .unwrap()
                .fingerprint()
                .unwrap()
        );
    }
    error(
        WorkerPluginExecutionPlan::canonicalize(input(vec![pin(&a), pin(&a)])),
        PluginCatalogError::Invalid,
    );
}

#[tokio::test]
async fn canonical_json_sorts_nested_config_and_selected_sets_without_copying_catalog_payloads() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let mut artifact = installed.installation.artifacts[0].clone();
    artifact.tools.push(PluginToolMetadata {
        name: "other".into(),
        ..artifact.tools[0].clone()
    });
    artifact.requested_grants = vec![
        PluginGrantMetadata::Logging,
        PluginGrantMetadata::Https { slot: "api".into() },
    ];
    artifact.connections = vec![PluginConnectionMetadata {
        name: "api".into(),
        methods: vec![PluginHttpMethod::Get],
        path_prefixes: vec!["/".into()],
    }];
    let mut first = projection();
    first.config = serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[{"z":0,"a":false}]}"#).unwrap();
    first.tools = vec!["other".into(), "example_echo".into()];
    first.grants = vec![
        PluginGrant::Https { slot: "api".into() },
        PluginGrant::Logging,
    ];
    first.bindings = vec![BindingPolicyReference {
        slot: "api".into(),
        binding_id: "binding".into(),
        policy_digest: digest("policy"),
    }];
    let mut second = first.clone();
    second.config = serde_json::from_str(r#"{"a":[{"a":false,"z":0}],"z":{"a":1,"b":2}}"#).unwrap();
    second.tools.reverse();
    second.grants.reverse();
    let construct = |projection| {
        plan(vec![
            WorkerPluginPin::from_artifact(
                installed.installation.installation_id.clone(),
                installed.operation_id.clone(),
                &artifact,
                projection,
            )
            .unwrap(),
        ])
    };
    let canonical = construct(first);
    assert_eq!(canonical, construct(second));
    let json = canonical.canonical_json().unwrap();
    for excluded in [
        "revocation_reason",
        "description",
        "input_schema",
        "output_schema",
        "dcdl",
        "artifact_url",
        "host_path",
        "secret_value",
        "provider_handle",
        "token",
        "package_bytes",
    ] {
        assert!(
            !json.contains(&format!("\"{excluded}\"")),
            "unexpected {excluded}"
        );
    }
    let mut unknown = serde_json::to_value(&canonical).unwrap();
    unknown["input"]["execution"]["token"] = Value::String("credential".into());
    assert!(serde_json::from_value::<WorkerPluginExecutionPlan>(unknown).is_err());
}

#[tokio::test]
async fn create_replay_is_immutable_across_restart_and_rejects_changed_input() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let plan = f.configure(&[&installed]);
    let receipt = f
        .store
        .plugin_plan_create("actor", "launch", &plan)
        .unwrap();
    assert_eq!(receipt.initial_pin_state, PlanPinState::Pending);
    assert_eq!(f.kind(), "pending");
    f.transition(
        &receipt.plan_id,
        PlanPinState::Pending,
        PlanPinState::Active,
    );
    let reopened = SqliteWorkspaceStore::open_for_test(f.dir.path().join("server.db")).unwrap();
    assert_eq!(
        reopened
            .plugin_plan_create("actor", "launch", &plan)
            .unwrap(),
        receipt
    );
    assert_eq!(
        reopened
            .plugin_plan_load("a", &receipt.plan_id)
            .unwrap()
            .pin_state,
        PlanPinState::Active
    );
    let mut changed = plan.input.clone();
    changed.profile.projection_digest = digest("new profile");
    error(
        reopened.plugin_plan_create(
            "actor",
            "launch",
            &WorkerPluginExecutionPlan::canonicalize(changed).unwrap(),
        ),
        PluginCatalogError::Conflict,
    );
    assert_eq!(f.count("plugin_execution_plans"), 1);
    assert_eq!(f.count("plugin_references"), 1);
    assert_eq!(
        reopened
            .plugin_plan_load("a", &receipt.plan_id)
            .unwrap()
            .plan,
        plan
    );
    error(
        reopened.plugin_plan_load("b", &receipt.plan_id),
        PluginCatalogError::NotFound,
    );
}

#[tokio::test]
async fn retained_pin_lifecycle_blocks_uninstall_and_released_replay_never_reacquires() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let plan = f.configure(&[&installed]);
    let receipt = f
        .store
        .plugin_plan_create("actor", "launch", &plan)
        .unwrap();
    error(
        uninstall(&f, &installed, "uninstall-pending"),
        PluginCatalogError::Referenced,
    );
    f.transition(
        &receipt.plan_id,
        PlanPinState::Pending,
        PlanPinState::Active,
    );
    assert_eq!(f.kind(), "active");
    error(
        uninstall(&f, &installed, "uninstall-active"),
        PluginCatalogError::Referenced,
    );
    f.transition(
        &receipt.plan_id,
        PlanPinState::Active,
        PlanPinState::Restorable,
    );
    assert_eq!(f.kind(), "restorable");
    error(
        uninstall(&f, &installed, "uninstall-restorable"),
        PluginCatalogError::Referenced,
    );
    f.transition(
        &receipt.plan_id,
        PlanPinState::Restorable,
        PlanPinState::Active,
    );
    f.transition(
        &receipt.plan_id,
        PlanPinState::Active,
        PlanPinState::Released,
    );
    f.transition(
        &receipt.plan_id,
        PlanPinState::Released,
        PlanPinState::Released,
    );
    assert_eq!(f.count("plugin_references"), 0);
    f.disable();
    uninstall(&f, &installed, "uninstall-released").unwrap();
    assert_eq!(
        f.store
            .plugin_plan_create("actor", "launch", &plan)
            .unwrap(),
        receipt
    );
    assert_eq!(f.count("plugin_references"), 0);
    error(
        f.store.plugin_plan_transition(
            "a",
            &receipt.plan_id,
            PlanPinState::Released,
            PlanPinState::Active,
        ),
        PluginCatalogError::Conflict,
    );
    error(
        f.store
            .plugin_plan_create("actor", "different-launch", &plan),
        PluginCatalogError::NotFound,
    );
    assert_eq!(f.count("plugin_execution_plans"), 1);
    f.store
        .with_conn(|conn| {
            assert!(
                conn.execute("UPDATE plugin_execution_plans SET plan_json=plan_json", [])
                    .is_err()
            );
            assert!(
                conn.execute("UPDATE plugin_execution_plans SET pin_state='pending'", [])
                    .is_err()
            );
            assert!(
                conn.execute("DELETE FROM plugin_execution_plans", [])
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn revoke_blocks_create_and_new_pin_kinds_but_not_release_or_historical_replay() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let plan = f.configure(&[&installed]);
    let receipt = f
        .store
        .plugin_plan_create("actor", "launch", &plan)
        .unwrap();
    f.store
        .plugin_revoke(
            "a",
            "actor",
            &installed.installation.installation_id,
            &PluginRevokeRequest {
                artifact_digest: installed.installation.artifacts[0].artifact_digest.clone(),
                expected_last_operation: installed.operation_id.clone(),
                reason: "revoked".into(),
                operation_key: "revoke".into(),
            },
        )
        .unwrap();
    assert_eq!(
        f.store
            .plugin_plan_create("actor", "launch", &plan)
            .unwrap(),
        receipt
    );
    error(
        f.store.plugin_plan_transition(
            "a",
            &receipt.plan_id,
            PlanPinState::Pending,
            PlanPinState::Active,
        ),
        PluginCatalogError::Conflict,
    );
    assert_eq!(f.kind(), "pending");
    error(
        f.store.plugin_plan_create("actor", "new-launch", &plan),
        PluginCatalogError::Conflict,
    );
    assert_eq!(f.count("plugin_execution_plans"), 1);
    f.transition(
        &receipt.plan_id,
        PlanPinState::Pending,
        PlanPinState::Released,
    );
    assert_eq!(f.count("plugin_references"), 0);
}

#[tokio::test]
async fn artifact_addition_does_not_reselect_or_invalidate_existing_plan_lifecycle() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let plan = f.configure(&[&installed]);
    let receipt = f
        .store
        .plugin_plan_create("actor", "launch", &plan)
        .unwrap();
    let upload = f
        .store
        .plugin_upload(&f.dir.path().join("blobs"), "a", "actor", &package("0.2.0"))
        .unwrap();
    f.store
        .plugin_add_artifact(
            "a",
            "actor",
            &installed.installation.installation_id,
            &PluginAddArtifactRequest {
                upload_id: upload.upload_id,
                expected_last_operation: installed.operation_id.clone(),
                operation_key: "update".into(),
            },
        )
        .unwrap();
    let active = f.transition(
        &receipt.plan_id,
        PlanPinState::Pending,
        PlanPinState::Active,
    );
    assert_eq!(active.plan, plan);
    assert_eq!(
        active.plan.plugins()[0].artifact_digest(),
        installed.installation.artifacts[0].artifact_digest
    );
    error(
        f.store
            .plugin_plan_create("actor", "new-with-stale-observation", &plan),
        PluginCatalogError::Conflict,
    );
}

#[tokio::test]
async fn owner_savepoint_and_outer_transaction_rollback_leave_no_partial_plan_or_pins() {
    let f = Fixture::new().await;
    let first = f.install("one");
    let second = f.install("two");
    let mut bad_input = f.configure(&[&first, &second]).input;
    let mut pins = bad_input.plugins.clone();
    pins.sort_by(|a, b| a.installation_id.cmp(&b.installation_id));
    pins[1].artifact_size += 1;
    bad_input.plugins = pins;
    let bad_plan = WorkerPluginExecutionPlan::canonicalize(bad_input).unwrap();
    f.store
        .with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            error(
                plugin_plan_create(&tx, "actor", "bad", &bad_plan),
                PluginCatalogError::Conflict,
            );
            // Even if the caller commits after a handled error, first pin is gone.
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(f.count("plugin_execution_plans"), 0);
    assert_eq!(f.count("plugin_references"), 0);
    let valid = f.configure(&[&first]);
    f.store
        .with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            plugin_plan_create(&tx, "actor", "outer-rollback", &valid)?;
            tx.rollback()?;
            Ok(())
        })
        .unwrap();
    assert_eq!(f.count("plugin_execution_plans"), 0);
    assert_eq!(f.count("plugin_references"), 0);
    // The same key is usable after rollback because neither domain fact committed.
    f.store
        .plugin_plan_create("actor", "outer-rollback", &valid)
        .unwrap();
}

#[tokio::test]
async fn cross_workspace_and_exact_digest_mismatches_fail_without_retaining_references() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let base = f.configure(&[&installed]);
    let mut other_workspace = base.input.clone();
    other_workspace.workspace_id = "b".into();
    error(
        f.store.plugin_plan_create(
            "actor",
            "cross",
            &WorkerPluginExecutionPlan::canonicalize(other_workspace).unwrap(),
        ),
        PluginCatalogError::NotFound,
    );
    for change_package in [true, false] {
        let mut changed = base.input.clone();
        if change_package {
            changed.plugins[0].package_digest = digest("wrong-package");
        } else {
            changed.plugins[0].artifact_digest = digest("wrong-artifact");
        }
        let result = f.store.plugin_plan_create(
            "actor",
            "digests",
            &WorkerPluginExecutionPlan::canonicalize(changed).unwrap(),
        );
        error(
            result,
            if change_package {
                PluginCatalogError::Conflict
            } else {
                PluginCatalogError::NotFound
            },
        );
    }
    assert_eq!(f.count("plugin_execution_plans"), 0);
    assert_eq!(f.count("plugin_references"), 0);
}

#[tokio::test]
async fn sqlite_write_fence_serializes_uninstall_and_pin_creation_across_connections() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let plan = f.configure(&[&installed]);
    let mut other = Connection::open(f.dir.path().join("server.db")).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    f.store
        .with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            plugin_plan_create(&tx, "actor", "launch", &plan)?;
            // A second writer cannot pass the catalog deletion fence before commit.
            assert!(
                other
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .is_err()
            );
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    let tx = other
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    assert!(tx.execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id='a' AND installation_id=?1", params![installed.installation.installation_id]).is_err());
    tx.rollback().unwrap();
    error(
        uninstall(&f, &installed, "after-committed-pin"),
        PluginCatalogError::Referenced,
    );
    // Opposite ordering: uninstall wins before a new plan's owner acquires pins.
    let unpinned = f.install("unpinned");
    let unpinned_plan = f.configure(&[&unpinned]);
    f.disable();
    uninstall(&f, &unpinned, "uninstall-first").unwrap();
    error(
        f.store
            .plugin_plan_create("actor", "after-uninstall", &unpinned_plan),
        PluginCatalogError::NotFound,
    );
}

impl Fixture {
    fn configure(&self, installed: &[&PluginOperationResponse]) -> WorkerPluginExecutionPlan {
        use crate::config_source::{ConfigCommitRequest, WorkspaceConfigSchemaProvider};
        use config_source::{ConfigTreeChange, VirtualPath, WorkspaceConfigSchemaBundle};
        let bundle = WorkspaceConfigSchemaBundle::compose([
            crate::plugin_settings::PluginConfigSchemaProvider
                .contribution()
                .unwrap(),
            crate::profile_settings::ProfileConfigSchemaProvider
                .contribution()
                .unwrap(),
        ])
        .unwrap();
        let state = self
            .store
            .ensure_workspace_config_materialized_with_schema("a", "1", bundle.clone())
            .unwrap();
        let mut enabled = serde_json::Map::new();
        let mut selections = Vec::new();
        for (index, item) in installed.iter().enumerate() {
            let a = &item.installation.artifacts[0];
            enabled.insert(
                format!("plugin_{index}"),
                serde_json::json!({
                    "installation_id": item.installation.installation_id,
                    "expected_last_operation": item.operation_id,
                    "package_digest": a.package_digest, "artifact_digest": a.artifact_digest,
                    "tools": ["example_echo"], "config": {},
                }),
            );
            selections.push(serde_json::json!({"installation_id":item.installation.installation_id,"tools":["example_echo"]}));
        }
        let value = serde_json::json!({"plugins":{"enabled": enabled}, "profile":{"entries":[{"selector":"project:tool","profile":{"plugins":{"selections":selections}}}]}});
        let path = VirtualPath::parse("main.dcdl").unwrap();
        let candidate = self
            .store
            .evaluate_workspace_config_candidate_with_schema(
                "a",
                &ConfigCommitRequest {
                    base_digest: state.snapshot.digest.clone(),
                    entrypoints: vec![path.clone()],
                    changes: vec![ConfigTreeChange::Update {
                        path: path.clone(),
                        expected_digest: state.snapshot.get(&path).unwrap().content_digest.clone(),
                        content: format!(
                            "{} as WorkspaceConfigSchema",
                            crate::plugin_settings::render_value(&value).unwrap()
                        ),
                    }],
                },
                bundle,
            )
            .unwrap();
        let state = self
            .store
            .commit_evaluated_workspace_config("a", &candidate)
            .unwrap();
        self.store
            .with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                crate::plugin_settings::resolve_worker_plugin_plan(
                    &tx,
                    "a",
                    &state.snapshot.digest,
                    "project:tool",
                    input(vec![]).execution,
                )
            })
            .unwrap()
    }
    fn disable(&self) {
        self.configure(&[]);
    }
}
