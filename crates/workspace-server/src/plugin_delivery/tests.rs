//! Owner-DB integration: production upload/install, schema-checked config commit,
//! plan resolution/creation and file-backed SQLite. Host authentication/transport
//! are outside this suite; the supplied Host identity is the adapter's boundary.
use super::*;
use crate::{
    Error,
    plugin_plan::{PluginPlanCreateReceipt, WorkerPluginExecutionPlan},
    store::{ControlPlaneStore, WorkspaceRecord},
};
use manifest::plugin;
use rusqlite::Connection;
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
    fn root(&self) -> std::path::PathBuf {
        self.dir.path().join("artifacts")
    }
    fn install(&self, alias: &str) -> PluginOperationResponse {
        let upload = self
            .store
            .plugin_upload(&self.root(), "a", "actor", &package("0.1.0"))
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
    fn create(&self, installed: &PluginOperationResponse, key: &str) -> PluginPlanCreateReceipt {
        let plan = self.configure(&[installed]);
        self.store.plugin_plan_create("actor", key, &plan).unwrap()
    }
    fn authorize(
        &self,
        request: &PluginArtifactDeliveryRequest,
    ) -> Result<PluginArtifactDeliveryMetadata> {
        self.store
            .plugin_delivery_authorize("a", &execution(), request)
    }
    fn bytes(&self, request: &PluginArtifactDeliveryRequest) -> Result<Vec<u8>> {
        self.store
            .plugin_delivery_bytes(&self.root(), "a", &execution(), request)
    }
    fn retained(&self, plan_id: &str) -> Vec<String> {
        self.store.with_conn(|conn| Ok(conn.prepare("SELECT kind FROM plugin_references WHERE workspace_id='a' AND owner=?1 ORDER BY kind")?
            .query_map([plan_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)).unwrap()
    }
    fn revoke(&self, installed: &PluginOperationResponse) {
        self.store
            .plugin_revoke(
                "a",
                "actor",
                &installed.installation.installation_id,
                &PluginRevokeRequest {
                    expected_last_operation: installed.operation_id.clone(),
                    operation_key: "revoke".into(),
                    artifact_digest: installed.installation.artifacts[0].artifact_digest.clone(),
                    reason: "owner revoked".into(),
                },
            )
            .unwrap();
    }
}
fn execution() -> HostExecutionIdentity {
    HostExecutionIdentity {
        runtime_id: "runtime".into(),
        worker_id: "worker".into(),
        principal_id: "worker-principal".into(),
        delegation_id: Some("delegation".into()),
    }
}
fn request(saved: &PluginPlanCreateReceipt) -> PluginArtifactDeliveryRequest {
    let pin = &saved.plan.plugins()[0];
    PluginArtifactDeliveryRequest {
        plan_id: saved.plan_id.clone(),
        plan_fingerprint: saved.fingerprint.clone(),
        installation_id: pin.installation_id().into(),
        artifact_digest: pin.artifact_digest().into(),
    }
}
fn error<T: std::fmt::Debug>(result: Result<T>, expected: PluginCatalogError) {
    assert!(
        matches!(result, Err(Error::PluginCatalog(actual)) if actual == expected),
        "expected {expected:?}, got {result:?}"
    );
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
                    execution(),
                )
            })
            .unwrap()
    }
    fn disable(&self) {
        self.configure(&[]);
    }
}

#[tokio::test]
async fn pending_active_restorable_reads_preserve_exact_pin_across_restart() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    let original = package("0.1.0");
    for (state, kind) in [
        (PlanPinState::Pending, "pending"),
        (PlanPinState::Active, "active"),
        (PlanPinState::Restorable, "restorable"),
    ] {
        let before = f.retained(&saved.plan_id);
        assert_eq!(before, [kind]);
        assert_eq!(
            f.authorize(&request).unwrap(),
            PluginArtifactDeliveryMetadata {
                workspace_id: "a".into(),
                request: request.clone(),
                artifact: installed.installation.artifacts[0].clone(),
            }
        );
        assert_eq!(f.bytes(&request).unwrap(), original);
        assert_eq!(f.retained(&saved.plan_id), before);
        assert_eq!(
            f.store
                .plugin_plan_load("a", &saved.plan_id)
                .unwrap()
                .pin_state,
            state
        );
        let reopened = SqliteWorkspaceStore::open_for_test(f.dir.path().join("server.db")).unwrap();
        assert_eq!(
            reopened
                .plugin_delivery_bytes(&f.root(), "a", &execution(), &request)
                .unwrap(),
            original
        );
        if state != PlanPinState::Restorable {
            f.store
                .plugin_plan_transition(
                    "a",
                    &saved.plan_id,
                    state,
                    if state == PlanPinState::Pending {
                        PlanPinState::Active
                    } else {
                        PlanPinState::Restorable
                    },
                )
                .unwrap();
        }
    }
}

#[tokio::test]
async fn foreign_workspace_host_identity_and_plan_coordinates_cannot_fetch_bytes() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let other = f.install("other");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    error(
        f.store
            .plugin_delivery_authorize("b", &execution(), &request),
        PluginCatalogError::NotFound,
    );
    error(
        f.store
            .plugin_delivery_bytes(&f.root(), "b", &execution(), &request),
        PluginCatalogError::NotFound,
    );
    let mut identities = Vec::new();
    let mut wrong = execution();
    wrong.runtime_id = "other-runtime".into();
    identities.push(wrong);
    let mut wrong = execution();
    wrong.worker_id = "other-worker".into();
    identities.push(wrong);
    let mut wrong = execution();
    wrong.principal_id = "other-principal".into();
    identities.push(wrong);
    let mut wrong = execution();
    wrong.delegation_id = Some("other-delegation".into());
    identities.push(wrong);
    let mut wrong = execution();
    wrong.delegation_id = None;
    identities.push(wrong);
    for wrong in identities {
        error(
            f.store.plugin_delivery_authorize("a", &wrong, &request),
            PluginCatalogError::Conflict,
        );
        error(
            f.store
                .plugin_delivery_bytes(&f.root(), "a", &wrong, &request),
            PluginCatalogError::Conflict,
        );
    }
    let mut cases = Vec::new();
    let mut wrong = request.clone();
    wrong.plan_id = "missing-plan".into();
    cases.push((wrong, PluginCatalogError::NotFound));
    let mut wrong = request.clone();
    wrong.plan_fingerprint = format!("sha256:{}", "0".repeat(64));
    cases.push((wrong, PluginCatalogError::Conflict));
    let mut wrong = request.clone();
    wrong.installation_id = other.installation.installation_id.clone();
    cases.push((wrong, PluginCatalogError::NotFound));
    let mut wrong = request.clone();
    wrong.artifact_digest = format!("sha256:{}", "0".repeat(64));
    cases.push((wrong, PluginCatalogError::NotFound));
    // Another real persisted plan cannot lend its fingerprint to this plan.
    let other_plan = f.create(&other, "other-launch");
    let mut wrong = request.clone();
    wrong.plan_id = other_plan.plan_id;
    cases.push((wrong, PluginCatalogError::Conflict));
    for (wrong, expected) in cases {
        error(f.authorize(&wrong), expected);
        error(f.bytes(&wrong), expected);
    }
    assert_eq!(f.retained(&saved.plan_id), ["pending"]);
}

#[tokio::test]
async fn revoked_artifact_denies_new_metadata_and_bytes_without_releasing_audit_pin() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    // Previously obtained metadata cannot stand in for current admission.
    let metadata = f.authorize(&request).unwrap();
    f.revoke(&installed);
    error(f.authorize(&metadata.request), PluginCatalogError::Conflict);
    error(f.bytes(&metadata.request), PluginCatalogError::Conflict);
    let stored = f.store.plugin_plan_load("a", &saved.plan_id).unwrap();
    assert_eq!(stored.plan, saved.plan);
    assert_eq!(stored.pin_state, PlanPinState::Pending);
    assert_eq!(f.retained(&saved.plan_id), ["pending"]);
}

#[tokio::test]
async fn released_plan_and_missing_or_wrong_kind_reference_never_authorize_delivery() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    // Retention corruption is injected after valid owner creation, not by
    // constructing or inserting a fake serialized plan.
    f.store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE plugin_references SET kind='active' WHERE workspace_id='a' AND owner=?1",
                [&saved.plan_id],
            )?;
            Ok(())
        })
        .unwrap();
    error(f.authorize(&request), PluginCatalogError::Conflict);
    error(f.bytes(&request), PluginCatalogError::Conflict);
    f.store
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM plugin_references WHERE workspace_id='a' AND owner=?1",
                [&saved.plan_id],
            )?;
            Ok(())
        })
        .unwrap();
    error(f.authorize(&request), PluginCatalogError::Conflict);
    error(f.bytes(&request), PluginCatalogError::Conflict);
    let another = f.create(&installed, "second-launch");
    let another_request = self::request(&another);
    f.store
        .plugin_plan_transition(
            "a",
            &another.plan_id,
            PlanPinState::Pending,
            PlanPinState::Released,
        )
        .unwrap();
    error(f.authorize(&another_request), PluginCatalogError::Conflict);
    error(f.bytes(&another_request), PluginCatalogError::Conflict);
    assert!(f.retained(&another.plan_id).is_empty());
    assert_eq!(
        f.store
            .plugin_plan_load("a", &another.plan_id)
            .unwrap()
            .plan,
        another.plan
    );
}

#[tokio::test]
async fn new_catalog_version_and_current_config_change_do_not_reselect_saved_pin() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    let original = f.bytes(&request).unwrap();
    let upload = f
        .store
        .plugin_upload(&f.root(), "a", "actor", &package("0.2.0"))
        .unwrap();
    let added = f
        .store
        .plugin_add_artifact(
            "a",
            "actor",
            &installed.installation.installation_id,
            &PluginAddArtifactRequest {
                upload_id: upload.upload_id,
                expected_last_operation: installed.operation_id.clone(),
                operation_key: "add-version".into(),
            },
        )
        .unwrap();
    assert_ne!(added.operation_id, installed.operation_id);
    f.disable();
    assert_eq!(
        f.authorize(&request).unwrap().artifact,
        installed.installation.artifacts[0]
    );
    assert_eq!(f.bytes(&request).unwrap(), original);
    assert_eq!(f.retained(&saved.plan_id), ["pending"]);
    let newer = added
        .installation
        .artifacts
        .iter()
        .find(|a| a.version == "0.2.0")
        .unwrap();
    let mut outside = request.clone();
    outside.artifact_digest = newer.artifact_digest.clone();
    error(f.authorize(&outside), PluginCatalogError::NotFound);
    error(f.bytes(&outside), PluginCatalogError::NotFound);
}

#[tokio::test]
async fn release_then_uninstall_keeps_plan_for_audit_but_denies_delivery() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    f.store
        .plugin_plan_transition(
            "a",
            &saved.plan_id,
            PlanPinState::Pending,
            PlanPinState::Released,
        )
        .unwrap();
    f.disable();
    f.store
        .plugin_uninstall(
            "a",
            "actor",
            &installed.installation.installation_id,
            &PluginUninstallRequest {
                expected_last_operation: installed.operation_id.clone(),
                operation_key: "uninstall".into(),
            },
        )
        .unwrap();
    error(f.authorize(&request), PluginCatalogError::Conflict);
    error(f.bytes(&request), PluginCatalogError::Conflict);
    assert_eq!(
        f.store.plugin_plan_load("a", &saved.plan_id).unwrap().plan,
        saved.plan
    );
}

#[tokio::test]
async fn artifact_metadata_identity_corruption_cannot_override_persisted_pin() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    let original = serde_json::to_value(&installed.installation.artifacts[0]).unwrap();
    let mut corruptions = Vec::new();
    for (name, value) in [
        (
            "artifact_digest",
            serde_json::json!(format!("sha256:{}", "0".repeat(64))),
        ),
        (
            "package_digest",
            serde_json::json!(format!("sha256:{}", "0".repeat(64))),
        ),
        ("package_id", serde_json::json!("other-package")),
        ("version", serde_json::json!("9.0.0")),
        ("artifact_size", serde_json::json!(1)),
        ("schema_version", serde_json::json!(999)),
        ("world", serde_json::json!("unsupported-world")),
        ("required_imports", serde_json::json!(["other-import"])),
    ] {
        let mut value_ = original.clone();
        value_[name] = value;
        corruptions.push(value_);
    }
    for name in ["identity", "version", "namespace", "digest"] {
        let mut changed = original.clone();
        changed["config_schema"][name] = if name == "digest" {
            serde_json::json!(format!("sha256:{}", "0".repeat(64)))
        } else {
            serde_json::json!("other-schema")
        };
        corruptions.push(changed);
    }
    for metadata in corruptions {
        f.store.with_conn(|conn| {
            conn.execute("UPDATE plugin_artifacts SET metadata_json=?1 WHERE workspace_id='a' AND installation_id=?2 AND artifact_digest=?3", params![metadata.to_string(), request.installation_id, request.artifact_digest])?;
            Ok(())
        }).unwrap();
        assert!(
            f.authorize(&request).is_err(),
            "admitted changed {metadata}"
        );
        assert!(f.bytes(&request).is_err(), "fetched changed {metadata}");
    }
    assert_eq!(f.retained(&saved.plan_id), ["pending"]);
}

#[tokio::test]
async fn bounded_blob_read_rejects_missing_truncated_oversize_and_digest_mismatch() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    let path = f
        .root()
        .join("blobs")
        .join(request.artifact_digest.strip_prefix("sha256:").unwrap());
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    error(f.bytes(&request), PluginCatalogError::Unavailable);
    for bad in [vec![], vec![0; original.len() + 1], vec![0; original.len()]] {
        fs::write(&path, bad).unwrap();
        error(f.bytes(&request), PluginCatalogError::Unavailable);
    }
    fs::write(&path, &original).unwrap();
    assert_eq!(f.bytes(&request).unwrap(), original);
    assert_eq!(f.retained(&saved.plan_id), ["pending"]);
}

#[tokio::test]
async fn owner_transaction_fences_concurrent_catalog_mutation_until_bytes_are_read() {
    let f = Fixture::new().await;
    let installed = f.install("one");
    let saved = f.create(&installed, "launch");
    let request = request(&saved);
    let mut other = Connection::open(f.dir.path().join("server.db")).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    f.store
        .with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            plugin_delivery_authorize(&tx, "a", &execution(), &request)?;
            assert!(
                other
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .is_err()
            );
            assert_eq!(
                plugin_delivery_bytes(&tx, &f.root(), "a", &execution(), &request)?,
                package("0.1.0")
            );
            tx.commit()?;
            Ok(())
        })
        .unwrap();
    other
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap()
        .rollback()
        .unwrap();
    f.revoke(&installed);
    error(f.bytes(&request), PluginCatalogError::Conflict);
}
