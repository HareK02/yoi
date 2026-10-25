use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

use config_source::{ConfigTreeChange, VirtualPath};
use protocol::SessionSnapshot;
use serde_json::Value;
use worker_runtime::FsRuntimeStoreOptions;
use worker_runtime::catalog::{
    ConfigBundleRef, CreateWorkerRequest, ProfileSelector, ProfileSourceArchiveSource,
    WorkspaceApiRef,
};
use yoi_e2e::{Result, WorkerE2eEnvironment};
use yoi_workspace_server::auth::{mint_secret, new_id, now_rfc3339, token_hash};
use yoi_workspace_server::config_source::{ConfigCommitRequest, WorkspaceConfigSchemaRegistry};
use yoi_workspace_server::hosts::{
    WorkerSpawnAcceptanceRequirement, WorkerSpawnIntent, WorkerSpawnRequest,
};
use yoi_workspace_server::profile_settings::{
    ProfileConfigSchemaProvider, build_virtual_profile_config_bundle,
    project_profiles_from_workspace_config,
};
use yoi_workspace_server::store::{
    AccountRecord, ApiTokenRecord, UserRecord, WorkspaceBootstrapRecord,
    WorkspaceRuntimeAuthenticationMode, WorkspaceRuntimeBinding,
};
use yoi_workspace_server::test_support::{FixedWorkerSeed, seed_fixed_worker};
use yoi_workspace_server::workspace_signing_identity::{
    FsWorkspaceSigningMaterialStore, WorkspaceSigningIdentityService,
    workspace_signing_material_root,
};
use yoi_workspace_server::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};

use super::fixture::{Fixture, PROFILE, failure, support};

pub struct Installed {
    pub environment: WorkerE2eEnvironment,
    pub snapshot: SessionSnapshot,
    pub runtime_public_key: String,
    pub workspace_public_key: String,
}

pub fn start(fixture: &Fixture) -> Result<Installed> {
    let mut environment = WorkerE2eEnvironment::new()?;
    super::phase(&environment, "fixture-copy-verify")?;
    let copy = environment.root.join("provided-fixture");
    fixture.source.copy_to(&copy)?;
    fs::write(
        environment.artifacts_dir.join("fixture-manifest.json"),
        fs::read(copy.join("manifest.json"))?,
    )?;
    let template = fixture.template(&copy)?;
    let runtime_identity =
        environment.initialize_runtime_identity(fixture.manifest.runtime_id.clone())?;
    let runtime_public_key = runtime_identity["public_key"]
        .as_str()
        .ok_or_else(|| failure("Runtime public bundle omitted key"))?
        .to_owned();

    // Reserve an environment-local placeholder endpoint only while the Runtime is
    // offline. Its real :0 endpoint replaces this binding through public API CAS.
    let reserved_endpoint = TcpListener::bind("127.0.0.1:0")?;
    super::phase(
        &environment,
        "offline-backend-bootstrap-profile-worker-seed",
    )?;
    let bootstrap = bootstrap(
        &environment,
        fixture,
        &copy,
        &runtime_identity,
        &format!("http://{}", reserved_endpoint.local_addr()?),
        &template.worker_name(),
    )?;
    // Close the placeholder before Backend probes it: an accepting TCP listener
    // with no HTTP service would stall discovery until its transport timeout.
    drop(reserved_endpoint);
    super::phase(&environment, "start-seeded-backend")?;
    environment.start_seeded_backend(
        fixture.manifest.workspace_id.clone(),
        fixture.manifest.runtime_id.clone(),
        bootstrap.token,
    )?;
    let mut snapshot = None;
    super::phase(&environment, "initialize-runtime-trust")?;
    environment.start_runtime_with_seed(|environment| {
        super::phase(environment, "install-runtime-fixture-and-validate-session")?;
        let options = FsRuntimeStoreOptions::new(environment.runtime_data_dir.join("runtime"))
            .with_runtime_id(&fixture.manifest.runtime_id);
        let request = CreateWorkerRequest {
            worker_id: template.worker_id(),
            create_fingerprint: bootstrap.create_fingerprint.clone(),
            profile: ProfileSelector::Named(PROFILE.into()),
            display_name: Some("Restore E2E fixture".into()),
            profile_source: ProfileSourceArchiveSource::WorkspaceConfig {
                archive: bootstrap.archive.clone(),
            },
            config_bundle: Some(bootstrap.config_bundle.clone()),
            workspace_api: Some(WorkspaceApiRef {
                workspace_id: fixture.manifest.workspace_id.clone(),
                base_url: environment.backend_target()?.0.into(),
            }),
            initial_input: None,
            workdir_attachment_requests: vec![],
            workdir_attachments: vec![],
            worker_observation_enabled: false,
            worker_observation_grants: vec![],
            memory_settings: Some(bootstrap.memory_settings.clone()),
            subjektiv_attached: false,
            backend_job: None,
        };
        support(template.install(&options, request, &bootstrap.manifest))?;
        let sessions = support(session_store::WorkerSessionStore::open_read_only(
            options
                .root
                .join("workers")
                .join(template.worker_id().to_string())
                .join("session"),
        ))?;
        let (logs, _) = support(sessions.read_all_read_only_bounded(
            template.session_id(),
            template.segment_id(),
            8 * 1024 * 1024,
        ))?;
        let projected = session_store::project_session_snapshot(template.session_id(), &logs);
        fixture.verify_conversation(&projected)?;
        snapshot = Some(full_restored_snapshot(&sessions, &logs));
        Ok(())
    })?;
    fixture.source.assert_unchanged()?;
    Ok(Installed {
        environment,
        snapshot: snapshot.ok_or_else(|| failure("Runtime seed was not invoked"))?,
        runtime_public_key,
        workspace_public_key: bootstrap.workspace_public_key,
    })
}

fn full_restored_snapshot(
    store: &dyn session_store::Store,
    logs: &[session_store::LogEntry],
) -> SessionSnapshot {
    session_store::public_snapshot::project_current_session_snapshot_with_accounting(store, logs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{
        Event, Greeting, InFlightSnapshot, InvokeKind, SessionSnapshotEntryData,
        WorkerStateSnapshot, WorkerStatus,
    };
    use session_store::{
        LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
        LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin,
    };

    #[test]
    fn completed_run_full_snapshot_retains_invoke_and_usage_accounting() -> Result<()> {
        // Projection regression only: no saved Worker or substitute E2E fixture.
        let temp = tempfile::tempdir()?;
        let store = support(session_store::WorkerSessionStore::new(temp.path()))?;
        let session_id = session_store::new_session_id();
        let logs = vec![
            LogEntry::AnnotatedSegmentStart {
                ts: 1,
                session_id,
                system_prompt: None,
                config: Default::default(),
                history: vec![],
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::Invoke {
                ts: 10,
                trigger: InvokeKind::UserSend,
            },
            LogEntry::AnnotatedAssistantItem {
                ts: 20,
                entry: LoggedHistoryEntry {
                    item: LoggedItem::Message {
                        role: LoggedRole::Assistant,
                        content: vec![LoggedContentPart::Text {
                            text: "unit response".into(),
                        }],
                    },
                    metadata: LoggedSessionHistoryMetadata {
                        entry_id: LoggedSessionHistoryEntryId("unit-response".into()),
                        origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                        derivation: None,
                        input_metadata: None,
                    },
                },
            },
            LogEntry::LlmUsage {
                ts: 30,
                history_len: 1,
                input_total_tokens: 100,
                cache_read_tokens: 20,
                cache_write_tokens: 0,
                output_tokens: 5,
            },
            serde_json::from_value(serde_json::json!({"kind":"run_completed", "ts":40,
                "interrupted":false,"result":"finished"}))?,
        ];
        let conversation = session_store::project_session_snapshot(session_id, &logs);
        let expected = full_restored_snapshot(&store, &logs);
        assert_eq!(expected.entries.len(), conversation.entries.len() + 2);
        assert!(matches!(
            expected.entries[expected.entries.len() - 2].data,
            SessionSnapshotEntryData::Invoke {
                trigger: InvokeKind::UserSend
            }
        ));
        assert_eq!(expected.entries[expected.entries.len() - 2].timestamp, 10);
        assert!(matches!(
            expected.entries.last().unwrap().data,
            SessionSnapshotEntryData::Usage {
                input_tokens: 100,
                cache_read_input_tokens: 20,
                output_tokens: 5
            }
        ));
        assert_eq!(expected.entries.last().unwrap().timestamp, 30);
        let live = session_store::public_snapshot::project_current_session_snapshot_with_accounting(
            &store, &logs,
        );
        let event = Event::Snapshot {
            session: live,
            state: WorkerStateSnapshot::from(WorkerStatus::Idle),
            in_flight: InFlightSnapshot::default(),
            internal_workers: vec![],
            greeting: Greeting {
                worker_name: "unit".into(),
                cwd: ".".into(),
                provider: "unit".into(),
                model: "unit".into(),
                reasoning: None,
                scope_summary: "none".into(),
                tools: vec![],
                context_window: 0,
                context_tokens: 0,
                context_usage: None,
            },
        };
        let raw = serde_json::to_value(event)?;
        yoi_e2e::validate_full_snapshot(&raw, &expected)?;
        assert!(yoi_e2e::validate_full_snapshot(&raw, &conversation).is_err());
        Ok(())
    }
}

struct Bootstrap {
    token: String,
    workspace_public_key: String,
    create_fingerprint: String,
    archive: worker_runtime::profile_archive::ProfileSourceArchiveRef,
    config_bundle: ConfigBundleRef,
    manifest: manifest::WorkerManifest,
    memory_settings: manifest::WorkspaceMemorySettingsSnapshot,
}

fn bootstrap(
    environment: &WorkerE2eEnvironment,
    fixture: &Fixture,
    copy: &Path,
    runtime_identity: &Value,
    endpoint: &str,
    worker_name: &str,
) -> Result<Bootstrap> {
    fs::create_dir_all(&environment.server_data_dir)?;
    let database = environment.server_data_dir.join("server.db");
    if database.exists() {
        return Err(failure("Backend seed refuses an existing database"));
    }
    let store = support(SqliteWorkspaceStore::open(&database))?;
    let now = now_rfc3339();
    let account_id = new_id("acct-user");
    let user_id = new_id("user");
    support(store.upsert_account(&AccountRecord {
        account_id: account_id.clone(),
        kind: "user".into(),
        handle: environment.account_handle.clone(),
        display_name: "Restore E2E owner".into(),
        created_at: now.clone(),
        updated_at: now.clone(),
    }))?;
    support(store.upsert_user(&UserRecord {
        user_id: user_id.clone(),
        account_id: account_id.clone(),
        handle: environment.account_handle.clone(),
        display_name: "Restore E2E owner".into(),
        created_at: now.clone(),
        updated_at: now.clone(),
    }))?;
    let token = mint_secret("yoi_api");
    support(store.create_api_token(&ApiTokenRecord {
        token_id: new_id("api-token"),
        token_hash: token_hash(&token),
        user_id,
        label: "restore-e2e".into(),
        created_at: now.clone(),
        expires_at: None,
        revoked_at: None,
        last_used_at: None,
    }))?;
    let identities = WorkspaceSigningIdentityService::new(
        Arc::new(store.clone()),
        Arc::new(FsWorkspaceSigningMaterialStore::new(
            workspace_signing_material_root(&database),
        )),
    );
    let (identity, operation) = support(identities.prepare_workspace_creation(
        "restore-e2e-bootstrap",
        "restore-e2e-bootstrap",
        &fixture.manifest.workspace_id,
        &account_id,
    ))?;
    let workspace_public_key = identity.public_key.clone();
    support(store.create_workspace_bootstrap(
        &WorkspaceBootstrapRecord {
            operation_key: "restore-e2e-bootstrap".into(),
            request_fingerprint: "restore-e2e-bootstrap".into(),
            workspace: WorkspaceRecord {
                workspace_id: fixture.manifest.workspace_id.clone(),
                owner_account_id: account_id,
                display_name: "Restore E2E".into(),
                state: "active".into(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            repository: None,
        },
        &identity,
        &operation,
    ))?;
    support(
        store.upsert_workspace_runtime_binding(
            WorkspaceRuntimeBinding {
                workspace_id: fixture.manifest.workspace_id.clone(),
                runtime_id: fixture.manifest.runtime_id.clone(),
                display_name: format!("E2E {}", fixture.manifest.runtime_id),
                base_url: endpoint.into(),
                public_key: runtime_identity["public_key"]
                    .as_str()
                    .ok_or_else(|| failure("missing Runtime public key"))?
                    .into(),
                public_key_fingerprint: String::new(),
                binding_id: String::new(),
                authentication_mode: WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity,
                created_at: now.clone(),
                updated_at: now.clone(),
                revoked_at: None,
            },
            false,
        ),
    )?;

    let schema = support(
        WorkspaceConfigSchemaRegistry::default()
            .with_provider(Arc::new(ProfileConfigSchemaProvider))
            .with_provider(Arc::new(
                yoi_workspace_server::prompt_settings::PromptConfigSchemaProvider,
            ))
            .with_provider(Arc::new(
                yoi_workspace_server::runtime_settings::RuntimeConfigSchemaProvider,
            ))
            .with_provider(Arc::new(
                yoi_workspace_server::repository_access::RepositoryAccessConfigSchemaProvider,
            ))
            .with_provider(Arc::new(
                yoi_workspace_server::skills::SkillConfigSchemaProvider,
            ))
            .compose(),
    )?;
    let state = support(store.ensure_workspace_config_materialized_with_schema(
        &fixture.manifest.workspace_id,
        &now,
        schema.clone(),
    ))?;
    let main = support(VirtualPath::parse("main.dcdl"))?;
    let current = state
        .snapshot
        .get(&main)
        .ok_or_else(|| failure("Workspace config omitted main.dcdl"))?;
    let candidate = support(store.evaluate_workspace_config_candidate_with_schema(
        &fixture.manifest.workspace_id,
        &ConfigCommitRequest {
            base_digest: state.snapshot.digest.clone(),
            changes: vec![ConfigTreeChange::Update {
                path: main.clone(),
                expected_digest: current.content_digest.clone(),
                content: fs::read_to_string(copy.join("workspace-config.dcdl"))?,
            }],
            entrypoints: vec![main],
        },
        schema,
    ))?;
    let state = support(
        store.commit_evaluated_workspace_config(&fixture.manifest.workspace_id, &candidate),
    )?;
    let projection = support(project_profiles_from_workspace_config(
        &fixture.manifest.workspace_id,
        &state,
    ))?;
    let bundle = support(build_virtual_profile_config_bundle(
        &projection,
        &state,
        &fixture.manifest.workspace_id,
        &now,
        PROFILE,
    ))?
    .ok_or_else(|| failure("fixture Profile is not registered"))?;
    let archive = bundle
        .profile_source_archive
        .as_ref()
        .ok_or_else(|| failure("fixture Profile archive is absent"))?;
    let mut resolved = support(
        support(archive.verify())?.resolve_profile(
            PROFILE,
            &environment
                .runtime_data_dir
                .join("runtime/workers")
                .join(&fixture.manifest.worker_id),
            worker_name,
        ),
    )?;
    if resolved.model.ref_.as_deref() != Some(fixture.manifest.model.as_str()) {
        return Err(failure("resolved Profile changed the supplied model"));
    }
    let request = WorkerSpawnRequest {
        intent: WorkerSpawnIntent::WorkspaceCoding,
        requested_worker_name: Some("Restore E2E fixture".into()),
        singleton_key: None,
        acceptance: WorkerSpawnAcceptanceRequirement::SocketReady,
        profile: ProfileSelector::Named(PROFILE.into()),
        ticket_assignment: None,
        initial_submit: vec![],
        workdir_attachment_requests: vec![],
        resolved_workdir_attachment_requests: vec![],
        resolved_workdir_attachments: vec![],
        resolved_config_bundle: None,
        resolved_workspace_api: None,
        resolved_memory_settings: None,
        resolved_subjektiv_attached: false,
        resolved_worker_observation_enabled: false,
        resolved_worker_observation_grants: vec![],
        resolved_control_operation: None,
        resolved_execution_grant: None,
    };
    let reservation = support(seed_fixed_worker(
        &store,
        FixedWorkerSeed {
            workspace_id: &fixture.manifest.workspace_id,
            runtime_id: &fixture.manifest.runtime_id,
            worker_id: support(fixture.manifest.worker_id.parse())?,
            allocation_key: "restore-e2e-worker",
            request: &request,
        },
    ))?;
    if resolved.feature.memory.profile.enabled {
        support(
            resolved
                .feature
                .memory
                .bind_workspace_settings(reservation.memory_settings.clone()),
        )?;
    }
    Ok(Bootstrap {
        manifest: resolved,
        memory_settings: reservation.memory_settings,
        token,
        workspace_public_key,
        create_fingerprint: reservation.create_fingerprint,
        archive: archive.reference.clone(),
        config_bundle: ConfigBundleRef {
            id: bundle.metadata.id,
            digest: bundle.metadata.digest,
        },
    })
}
