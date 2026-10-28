//! Real catalog + virtual config + SQLite transactions. HTTP authentication and
//! guest execution are outside this suite; the canonical Server config path is used.
use super::*;
use crate::config_source::{ConfigCommitRequest, WorkspaceConfigState};
use crate::store::{ControlPlaneStore, WorkspaceRecord};
use config_source::{ConfigTreeChange, VirtualPath, WorkspaceConfigSchemaBundle};
use serde_json::json;
use server_api::*;
use tempfile::{TempDir, tempdir};

struct Fixture {
    dir: TempDir,
    store: crate::SqliteWorkspaceStore,
    installed: PluginOperationResponse,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempdir().unwrap();
        let store = crate::SqliteWorkspaceStore::open_for_test(dir.path().join("db")).unwrap();
        for id in ["a", "b"] {
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: id.into(),
                    owner_account_id: "owner-account".into(),
                    display_name: id.into(),
                    state: "active".into(),
                    created_at: "1".into(),
                    updated_at: "1".into(),
                })
                .await
                .unwrap();
        }
        let upload = store
            .plugin_upload(
                &dir.path().join("artifacts"),
                "a",
                "owner",
                &package("0.1.0"),
            )
            .unwrap();
        let installed = store
            .plugin_install(
                "a",
                "owner",
                &PluginInstallRequest {
                    upload_id: upload.upload_id,
                    alias: "echo".into(),
                    operation_key: "install".into(),
                },
            )
            .unwrap();
        for id in ["a", "b"] {
            store
                .ensure_workspace_config_materialized_with_schema(id, "1", bundle())
                .unwrap();
        }
        Self {
            dir,
            store,
            installed,
        }
    }
    fn source(&self) -> serde_json::Value {
        let a = &self.installed.installation.artifacts[0];
        json!({"plugins":{"enabled":{"echo":{
            "installation_id":self.installed.installation.installation_id,
            "expected_last_operation":self.installed.operation_id,
            "package_digest":a.package_digest,"artifact_digest":a.artifact_digest,
            "tools":["example_echo"],"logging":false,"connections":{},"config":{"mode":"hello"}
        }},"bindings":{}},"profile":{"default_profile":"project:tool","entries":[{"selector":"project:tool","profile":{"plugins":{"selections":[{"installation_id":self.installed.installation.installation_id,"tools":["example_echo"],"logging":false}]}},"label":"","description":""}]}})
    }
    fn candidate(
        &self,
        workspace: &str,
        value: serde_json::Value,
    ) -> Result<crate::config_source::EvaluatedConfigCandidate> {
        let state = self.store.load_workspace_config(workspace)?.unwrap();
        let path = VirtualPath::parse("main.dcdl").unwrap();
        let request = ConfigCommitRequest {
            base_digest: state.snapshot.digest.clone(),
            entrypoints: vec![path.clone()],
            changes: vec![ConfigTreeChange::Update {
                path: path.clone(),
                expected_digest: state.snapshot.get(&path).unwrap().content_digest.clone(),
                content: format!("{} as WorkspaceConfigSchema", render_value(&value)?),
            }],
        };
        self.store
            .evaluate_workspace_config_candidate_with_schema(workspace, &request, bundle())
    }
    fn commit(&self, value: serde_json::Value) -> WorkspaceConfigState {
        let c = self.candidate("a", value).unwrap();
        self.store
            .commit_evaluated_workspace_config("a", &c)
            .unwrap()
    }
    fn refs(&self) -> i64 {
        self.store
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM plugin_references WHERE kind='config'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }
    fn plan(&self, state: &WorkspaceConfigState) -> crate::plugin_plan::WorkerPluginExecutionPlan {
        self.store
            .with_conn(|c| {
                let tx = c.unchecked_transaction()?;
                resolve_worker_plugin_plan(
                    &tx,
                    "a",
                    &state.snapshot.digest,
                    "project:tool",
                    crate::plugin_plan::HostExecutionIdentity {
                        runtime_id: "runtime".into(),
                        worker_id: "worker".into(),
                        principal_id: "principal".into(),
                        delegation_id: None,
                    },
                )
            })
            .unwrap()
    }
}
fn bundle() -> WorkspaceConfigSchemaBundle {
    WorkspaceConfigSchemaBundle::compose([
        PluginConfigSchemaProvider.contribution().unwrap(),
        crate::profile_settings::ProfileConfigSchemaProvider
            .contribution()
            .unwrap(),
    ])
    .unwrap()
}
fn package(version: &str) -> Vec<u8> {
    use manifest::plugin::*;
    let d = tempdir().unwrap();
    let source =
        include_str!("../../../../resources/plugin/templates/rust-component-tool/plugin.toml")
            .replace("version = \"0.1.0\"", &format!("version = \"{version}\""))
            .replace(
                "config_schema = \"{}\"",
                "config_schema = \"{ mode = String; }\"",
            );
    std::fs::write(d.path().join("plugin.toml"), source).unwrap();
    std::fs::write(
        d.path().join("plugin.component.wasm"),
        include_bytes!("../../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
    )
    .unwrap();
    let parsed = read_plugin_directory(
        d.path(),
        PluginSourceKind::Explicit,
        &PluginPackageLimits::default(),
    )
    .unwrap();
    pack_plugin_package(&parsed, &PluginPackageLimits::default())
        .unwrap()
        .bytes
}

#[tokio::test]
async fn config_two_phase_closed_schema_and_profile_narrowing_reject_unknown_abstract_and_expansion()
 {
    let f = Fixture::new().await;
    let source = f.source();
    let candidate = f.candidate("a", source.clone()).unwrap();
    assert!(
        candidate
            .contract
            .schema_bundle
            .source
            .contains("mode = String")
    );
    let native = SnapshotEnvironment::new(candidate.snapshot.clone())
        .evaluate_contract(&candidate.contract)
        .unwrap();
    assert_eq!(native, candidate.evaluation);
    for pointer in [
        "/plugins/enabled/echo/config/extra",
        "/plugins/enabled/echo/logging",
        "/profile/entries/0/profile/plugins/selections/0/logging",
    ] {
        let mut bad = source.clone();
        if pointer.ends_with("extra") {
            bad["plugins"]["enabled"]["echo"]["config"]["extra"] = json!(true);
        } else {
            *bad.pointer_mut(pointer)
                .unwrap_or_else(|| panic!("missing {pointer}")) = json!(true);
        }
        assert!(f.candidate("a", bad).is_err(), "accepted {pointer}");
    }
    let mut invalid = source.clone();
    invalid["plugins"]["enabled"]["echo"]["config"]["mode"] = json!(3);
    assert!(f.candidate("a", invalid).is_err());
    assert!(f.candidate("b", source).is_err());
    assert_eq!(f.refs(), 0);
}

#[tokio::test]
async fn config_refs_commit_atomically_block_uninstall_and_release_on_disable() {
    let f = Fixture::new().await;
    f.commit(f.source());
    assert_eq!(f.refs(), 1);
    assert!(
        f.store
            .plugin_uninstall(
                "a",
                "owner",
                &f.installed.installation.installation_id,
                &PluginUninstallRequest {
                    expected_last_operation: f.installed.operation_id.clone(),
                    operation_key: "remove".into()
                }
            )
            .is_err()
    );
    f.commit(json!({"plugins":{},"profile":{"entries":[]}}));
    assert_eq!(f.refs(), 0);
    f.store
        .plugin_uninstall(
            "a",
            "owner",
            &f.installed.installation.installation_id,
            &PluginUninstallRequest {
                expected_last_operation: f.installed.operation_id.clone(),
                operation_key: "remove-after-disable".into(),
            },
        )
        .unwrap();
}

#[tokio::test]
async fn catalog_add_after_preview_rejects_stale_candidate_and_preserves_config_refs() {
    let f = Fixture::new().await;
    let candidate = f.candidate("a", f.source()).unwrap();
    let upload = f
        .store
        .plugin_upload(
            &f.dir.path().join("artifacts"),
            "a",
            "owner",
            &package("0.2.0"),
        )
        .unwrap();
    f.store
        .plugin_add_artifact(
            "a",
            "owner",
            &f.installed.installation.installation_id,
            &PluginAddArtifactRequest {
                upload_id: upload.upload_id,
                expected_last_operation: f.installed.operation_id.clone(),
                operation_key: "update".into(),
            },
        )
        .unwrap();
    assert!(
        f.store
            .commit_evaluated_workspace_config("a", &candidate)
            .is_err()
    );
    assert_eq!(f.refs(), 0);
}

#[tokio::test]
async fn immutable_plan_create_replay_and_config_changes_do_not_hot_reload() {
    use crate::plugin_plan::*;
    let f = Fixture::new().await;
    let state = f.commit(f.source());
    let plan = f.plan(&state);
    assert_eq!(plan, f.plan(&state));
    let receipt = f.store.plugin_plan_create("host", "create", &plan).unwrap();
    let json = plan.canonical_json().unwrap();
    assert!(!json.contains("https://"));
    assert!(!json.contains(f.dir.path().to_str().unwrap()));
    let mut source = f.source();
    source["profile"]["entries"][0]["profile"]["plugins"]["selections"][0]["tools"] = json!([]);
    f.commit(source);
    assert_eq!(
        f.store.plugin_plan_create("host", "create", &plan).unwrap(),
        receipt
    );
    let saved = f.store.plugin_plan_load("a", &receipt.plan_id).unwrap();
    assert_eq!(saved.plan, plan);
    let reopened = crate::SqliteWorkspaceStore::open_for_test(f.dir.path().join("db")).unwrap();
    assert_eq!(
        reopened.plugin_plan_load("a", &receipt.plan_id).unwrap(),
        saved
    );
    reopened
        .plugin_plan_transition(
            "a",
            &receipt.plan_id,
            PlanPinState::Pending,
            PlanPinState::Released,
        )
        .unwrap();
    assert_eq!(
        reopened
            .plugin_plan_create("host", "create", &plan)
            .unwrap(),
        receipt
    );
}

#[tokio::test]
async fn profile_override_is_closed_schema_validated_and_plan_only_uses_effective_config() {
    let f = Fixture::new().await;
    let mut source = f.source();
    source["profile"]["entries"][0]["profile"]["plugins"]["selections"][0]["config"] =
        json!({"mode":"override"});
    let state = f.commit(source.clone());
    let plan = f.plan(&state);
    assert_eq!(
        plan.plugins()[0].projection().config,
        json!({"mode":"override"})
    );
    source["profile"]["entries"][0]["profile"]["plugins"]["selections"][0]["config"] =
        json!({"unknown":"not permitted"});
    assert!(f.candidate("a", source).is_err());
}

#[tokio::test]
async fn public_create_rejects_stale_or_forged_config_profile_and_projection_without_pins() {
    use crate::plugin_plan::WorkerPluginExecutionPlan;
    let f = Fixture::new().await;
    let state = f.commit(f.source());
    let plan = f.plan(&state);
    for pointer in [
        "/input/config/content_digest",
        "/input/profile/projection_digest",
        "/input/plugins/0/config_digest",
    ] {
        let mut forged = serde_json::to_value(&plan).unwrap();
        *forged.pointer_mut(pointer).unwrap() = json!(config_source::digest_bytes(b"forged"));
        let forged: WorkerPluginExecutionPlan = serde_json::from_value(forged).unwrap();
        assert!(
            f.store
                .plugin_plan_create("host", pointer, &forged)
                .is_err()
        );
    }
    let mut forged = serde_json::to_value(&plan).unwrap();
    forged["input"]["plugins"][0]["projection"]["config"] = json!({"mode":"caller-injected"});
    let forged: WorkerPluginExecutionPlan = serde_json::from_value(forged).unwrap();
    assert!(
        f.store
            .plugin_plan_create("host", "value-injection", &forged)
            .is_err()
    );
    let mut next = f.source();
    next["profile"]["entries"][0]["profile"]["plugins"]["selections"][0]["tools"] = json!([]);
    f.commit(next);
    assert!(
        f.store
            .plugin_plan_create("host", "stale-plan", &plan)
            .is_err()
    );
    f.store
        .with_conn(|c| {
            assert_eq!(
                c.query_row("SELECT count(*) FROM plugin_execution_plans", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM plugin_references WHERE kind != 'config'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn config_persistence_failure_rolls_back_reference_changes_and_restart_keeps_management_after_revoke()
 {
    let f = Fixture::new().await;
    let before = f.store.load_workspace_config("a").unwrap().unwrap();
    let candidate = f.candidate("a", f.source()).unwrap();
    f.store.with_conn(|c| { c.execute_batch("CREATE TRIGGER stop_config BEFORE INSERT ON workspace_config_entries BEGIN SELECT RAISE(ABORT,'config failure'); END;")?; Ok(()) }).unwrap();
    assert!(
        f.store
            .commit_evaluated_workspace_config("a", &candidate)
            .is_err()
    );
    assert_eq!(f.refs(), 0);
    assert_eq!(f.store.load_workspace_config("a").unwrap().unwrap(), before);
    f.store
        .with_conn(|c| {
            c.execute_batch("DROP TRIGGER stop_config")?;
            Ok(())
        })
        .unwrap();
    let committed = f
        .store
        .commit_evaluated_workspace_config("a", &candidate)
        .unwrap();
    let a = &f.installed.installation.artifacts[0];
    f.store
        .plugin_revoke(
            "a",
            "owner",
            &f.installed.installation.installation_id,
            &PluginRevokeRequest {
                artifact_digest: a.artifact_digest.clone(),
                expected_last_operation: f.installed.operation_id.clone(),
                reason: "withdraw".into(),
                operation_key: "withdraw".into(),
            },
        )
        .unwrap();
    let reopened = crate::SqliteWorkspaceStore::open_for_test(f.dir.path().join("db")).unwrap();
    assert_eq!(
        reopened
            .ensure_workspace_config_materialized_with_schema("a", "2", bundle())
            .unwrap(),
        committed
    );
    assert!(f.candidate("a", f.source()).is_err());
    f.commit(json!({"plugins":{},"profile":{"entries":[]}}));
    assert_eq!(f.refs(), 0);
}

#[tokio::test]
async fn https_config_binding_is_validated_pinned_by_digest_and_never_copied_to_plan() {
    use crate::plugin_plan::WorkerPluginExecutionPlan;
    use manifest::plugin::*;
    let f = Fixture::new().await;
    let d = tempdir().unwrap();
    std::fs::write(
        d.path().join("plugin.toml"),
        include_str!("../../../../resources/plugin/templates/rust-component-https/plugin.toml"),
    )
    .unwrap();
    std::fs::write(
        d.path().join("plugin.component.wasm"),
        include_bytes!("../../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm"),
    )
    .unwrap();
    let limits = PluginPackageLimits::default();
    let bytes = pack_plugin_package(
        &read_plugin_directory(d.path(), PluginSourceKind::Explicit, &limits).unwrap(),
        &limits,
    )
    .unwrap()
    .bytes;
    let upload = f
        .store
        .plugin_upload(&f.dir.path().join("artifacts"), "a", "owner", &bytes)
        .unwrap();
    let installed = f
        .store
        .plugin_install(
            "a",
            "owner",
            &PluginInstallRequest {
                upload_id: upload.upload_id,
                alias: "https".into(),
                operation_key: "https-install".into(),
            },
        )
        .unwrap();
    let artifact = &installed.installation.artifacts[0];
    let mut source = f.source();
    source["plugins"]["enabled"]["echo"] = json!({"installation_id":installed.installation.installation_id,"expected_last_operation":installed.operation_id,"package_digest":artifact.package_digest,"artifact_digest":artifact.artifact_digest,"tools":["example_status"],"connections":{"api":"remote"},"config":{}});
    source["plugins"]["bindings"] = json!({"remote":{"origin":"https://api.example.com","methods":["GET"],"path_prefixes":["/status"],"request_headers":["accept"],"response_headers":["content-type"],"secret_ref":"credential_ref","limits":{"timeout_ms":1000,"max_request_bytes":1024,"max_response_bytes":1024,"max_header_bytes":512}}});
    source["profile"]["entries"][0]["profile"]["plugins"]["selections"] = json!([{"installation_id":installed.installation.installation_id,"tools":["example_status"],"https_slots":["api"]}]);
    let state = f.commit(source.clone());
    let plan = f.plan(&state);
    let encoded = plan.canonical_json().unwrap();
    for excluded in [
        "https://api.example.com",
        "credential_ref",
        "origin",
        "request_headers",
        "secret_ref",
    ] {
        assert!(!encoded.contains(excluded), "leaked {excluded}");
    }
    assert_eq!(
        plan.plugins()[0].projection().bindings[0].binding_id,
        "remote"
    );
    let receipt = f
        .store
        .plugin_plan_create("host", "https-plan", &plan)
        .unwrap();
    let mut forged = serde_json::to_value(&plan).unwrap();
    forged["input"]["plugins"][0]["projection"]["bindings"][0]["policy_digest"] =
        json!(config_source::digest_bytes(b"forged-policy"));
    let forged: WorkerPluginExecutionPlan = serde_json::from_value(forged).unwrap();
    assert!(
        f.store
            .plugin_plan_create("host", "forged-policy", &forged)
            .is_err()
    );
    for path in ["/statusish", "/%2e%2e/private"] {
        let mut bad = source.clone();
        bad["plugins"]["bindings"]["remote"]["path_prefixes"] = json!([path]);
        assert!(f.candidate("a", bad).is_err());
    }
    source["plugins"]["bindings"]["remote"]["origin"] = json!("https://other.example.com");
    f.commit(source);
    assert!(
        f.store
            .plugin_plan_create("host", "stale-binding", &plan)
            .is_err()
    );
    assert_eq!(
        f.store
            .plugin_plan_create("host", "https-plan", &plan)
            .unwrap(),
        receipt
    );
}
