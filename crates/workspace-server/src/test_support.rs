//! Offline Backend fixture preparation, available only with `test-support`.
//!
//! Use a new isolated database and the existing account, Workspace signing-identity /
//! `ControlPlaneStore::create_workspace_bootstrap`, and Runtime binding APIs first.
//! Generate fresh account credentials and signing keys for each environment; no fixture
//! credentials are accepted here. Install Profile configuration through the existing
//! config-source mechanisms before starting the product processes.
//!
//! This module seeds Backend ownership only. It neither writes Runtime storage nor
//! launches a Worker, fabricates Runtime observations, changes authorization, or sends
//! initial input. The caller must separately install the matching stopped Runtime
//! fixture. Build product binaries in a separate Cargo invocation **without** this
//! feature (Cargo features unify within an invocation).

use super::*;
use crate::hosts::{WorkerSpawnIntent, WorkerSpawnRequest, worker_spawn_create_fingerprint};
use worker_runtime::catalog::ProfileSelector;

/// One ordinary, attachment-free Worker from an externally validated fixture.
///
/// IDs must match the Runtime fixture. `request` is the unresolved canonical create
/// intent, not a Runtime launch request containing credentials or execution grants.
/// The Profile selector must already be installed by the harness. `initial_submit`
/// must be empty: saved Session contents belong to the Runtime fixture, not new input.
/// Singleton/service, Ticket-assigned and attachment-bearing Workers are deliberately
/// unsupported.
pub struct FixedWorkerSeed<'a> {
    pub workspace_id: &'a str,
    pub runtime_id: &'a str,
    pub worker_id: WorkerId,
    pub allocation_key: &'a str,
    pub request: &'a WorkerSpawnRequest,
}

/// Insert a fixed Worker catalog entry and completed creation reservation atomically.
///
/// Uses the normal reservation validators, authoritative Memory snapshot, fingerprint
/// binding, resource-key allocator and immediate transaction. Even an exact repeat is
/// rejected: this is not an upsert/import/recovery API. Existing allocations, Worker
/// identities and residual lifecycle/attachment records are never adopted or repaired.
/// Returns the creation binding needed to check the corresponding Runtime fixture.
pub fn seed_fixed_worker(
    store: &SqliteWorkspaceStore,
    seed: FixedWorkerSeed<'_>,
) -> Result<WorkerCreateReservation> {
    validate_identifier("workspace_id", seed.workspace_id)?;
    validate_identifier("runtime_id", seed.runtime_id)?;
    let request = seed.request;
    if !request.initial_submit.is_empty() {
        return Err(Error::InvalidInput(
            "fixture Worker cannot contain initial_submit; offline seeding accepts no new input"
                .into(),
        ));
    }
    if !matches!(request.intent, WorkerSpawnIntent::WorkspaceCoding)
        || request.singleton_key.is_some()
        || request.ticket_assignment.is_some()
        || request.resolved_subjektiv_attached
        || !request.workdir_attachment_requests.is_empty()
        || !request.resolved_workdir_attachment_requests.is_empty()
        || !request.resolved_workdir_attachments.is_empty()
        || request.resolved_config_bundle.is_some()
        || request.resolved_workspace_api.is_some()
        || request.resolved_memory_settings.is_some()
        || request.resolved_worker_observation_enabled
        || !request.resolved_worker_observation_grants.is_empty()
        || request.resolved_control_operation.is_some()
        || request.resolved_execution_grant.is_some()
    {
        return Err(Error::InvalidInput(
            "fixture Worker must be ordinary unresolved create intent without singleton, assignments, attachments or grants".into(),
        ));
    }
    // Builtin selectors already carry their canonical `builtin:` prefix.
    let profile = match &request.profile {
        ProfileSelector::Builtin(name) | ProfileSelector::Named(name) => name,
    };
    validate_non_empty("fixture Profile selector", profile)?;
    let worker_id = seed.worker_id.to_string();
    let display_name = request
        .requested_worker_name
        .as_deref()
        .unwrap_or(&worker_id);
    validate_non_empty("fixture Worker display name", display_name)?;
    let fingerprint = worker_spawn_create_fingerprint(request).map_err(Error::InvalidInput)?;
    let memory = store.get_workspace_memory_settings(seed.workspace_id)?;
    store.reserve_worker_create_with(
        seed.workspace_id,
        seed.runtime_id,
        seed.allocation_key,
        &fingerprint,
        None,
        &memory,
        |tx| {
            require_seed_target(tx, &seed)?;
            Ok(Some(seed.worker_id))
        },
        |tx, reservation| {
            let now = chrono::Utc::now().to_rfc3339();
            tx.execute(
                "INSERT INTO worker_registry (
                    workspace_id, runtime_id, worker_id, display_name, profile,
                    retention_state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'normal', ?6, ?6)",
                params![seed.workspace_id, seed.runtime_id, worker_id, display_name, profile, now],
            )?;
            let changed = tx.execute(
                "UPDATE worker_create_reservations SET state = 'created', updated_at = ?4
                 WHERE workspace_id = ?1 AND runtime_id = ?2 AND worker_id = ?3 AND state = 'reserved'",
                params![seed.workspace_id, seed.runtime_id, reservation.worker_id.to_string(), now],
            )?;
            if changed != 1 {
                return Err(Error::Store("fixture creation reservation disappeared".into()));
            }
            Ok(())
        },
    )
}

fn require_seed_target(tx: &rusqlite::Transaction<'_>, seed: &FixedWorkerSeed<'_>) -> Result<()> {
    let active: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces w
         JOIN workspace_runtime_bindings r ON r.workspace_id = w.workspace_id
         JOIN workspace_signing_identities s ON s.workspace_id = w.workspace_id
         WHERE w.workspace_id = ?1 AND w.state = 'active'
           AND r.runtime_id = ?2 AND r.revoked_at IS NULL
           AND r.authentication_mode = 'workspace_identity' AND s.state = 'active')",
        params![seed.workspace_id, seed.runtime_id],
        |row| row.get(0),
    )?;
    if !active {
        return Err(Error::InvalidInput(
            "fixture requires an active bootstrapped Workspace and authenticated Runtime binding"
                .into(),
        ));
    }
    let worker_id = seed.worker_id.to_string();
    let allocation_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_create_reservations
         WHERE workspace_id = ?1 AND allocation_key = ?2)",
        params![seed.workspace_id, seed.allocation_key],
        |row| row.get(0),
    )?;
    if allocation_exists {
        return Err(Error::InvalidInput(
            "fixture Worker allocation already exists".into(),
        ));
    }
    // Check by WorkerId, not just the proposed Runtime tuple: orphaned and terminal
    // ownership is still a collision. These tables are owned by this store schema.
    for table in [
        "worker_create_reservations",
        "worker_registry",
        "worker_registry_observations",
        "worker_registry_projection_removals",
        "worker_removal_operations",
        "worker_singleton_owners",
        "worker_workdir_links",
        "worker_restore_intents",
        "worker_tombstones",
        "worker_orphan_diagnostics",
        "worker_registry_projection_diagnostics",
        "worker_session_archives",
        "worker_diagnostics_archives",
        "ticket_assignment_worker_tombstones",
    ] {
        let exists: bool = tx.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE worker_id = ?1)"),
            [&worker_id],
            |row| row.get(0),
        )?;
        if exists {
            return Err(Error::InvalidInput(format!(
                "fixture Worker identity already exists in {table}"
            )));
        }
    }
    // A resource key without a catalog/reservation must not be silently reused.
    let mut statement = tx.prepare(
        "SELECT resource_id FROM workspace_resource_keys WHERE resource_kind = 'worker'",
    )?;
    let resources = statement.query_map([], |row| row.get::<_, String>(0))?;
    for resource in resources {
        let resource = resource?;
        let existing = resource
            .is_ascii()
            .then(|| parse_worker_resource_id(&resource))
            .flatten()
            .ok_or_else(|| {
                Error::InvalidInput("unknown fixture Worker resource identity collision".into())
            })?;
        if existing.worker_id == worker_id {
            return Err(Error::InvalidInput(
                "fixture Worker resource identity already exists".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::{
        WorkerSpawnAcceptanceRequirement, WorkerSpawnWorkingDirectoryAttachmentRequest,
        WorkerSpawnWorkingDirectoryRequest,
    };
    use crate::workspace_signing_identity::{
        InMemoryWorkspaceSigningMaterialStore, WorkspaceSigningIdentityService,
    };

    const WORKSPACE: &str = "0197a949-4b6b-7f2a-9d9a-1f87e3a4c501";
    const RUNTIME: &str = "fixture-runtime";
    const WORKER: &str = "0197a949-4b6b-7f2a-9d9a-1f87e3a4c502";

    // Exercises the same public bootstrap APIs the offline harness uses. All account
    // and key identities are newly generated, even across stores with fixed fixture IDs.
    fn bootstrap() -> SqliteWorkspaceStore {
        let store = SqliteWorkspaceStore::in_memory_for_test().unwrap();
        let account = AccountRecord {
            account_id: format!("fixture-account-{}", Uuid::now_v7()),
            kind: "user".into(),
            handle: format!("fixture-{}", Uuid::now_v7()),
            display_name: "Fixture owner".into(),
            created_at: "2026-10-24T00:00:00Z".into(),
            updated_at: "2026-10-24T00:00:00Z".into(),
        };
        store.upsert_account(&account).unwrap();
        let identities = WorkspaceSigningIdentityService::new(
            Arc::new(store.clone()),
            Arc::new(InMemoryWorkspaceSigningMaterialStore::default()),
        );
        let (identity, operation) = identities
            .prepare_workspace_creation(
                "fixture-bootstrap",
                "fixture-bootstrap",
                WORKSPACE,
                &account.account_id,
            )
            .unwrap();
        store
            .create_workspace_bootstrap(
                &WorkspaceBootstrapRecord {
                    operation_key: "fixture-bootstrap".into(),
                    request_fingerprint: "fixture-bootstrap".into(),
                    workspace: WorkspaceRecord {
                        workspace_id: WORKSPACE.into(),
                        owner_account_id: account.account_id,
                        display_name: "Fixture".into(),
                        state: "active".into(),
                        created_at: account.created_at,
                        updated_at: account.updated_at,
                    },
                    repository: None,
                },
                &identity,
                &operation,
            )
            .unwrap();
        let runtime_key = worker_runtime::auth::RuntimeIdentityMaterial::generate(RUNTIME).unwrap();
        store
            .upsert_workspace_runtime_binding(
                WorkspaceRuntimeBinding {
                    workspace_id: WORKSPACE.into(),
                    runtime_id: RUNTIME.into(),
                    display_name: "Fixture Runtime".into(),
                    base_url: "http://127.0.0.1:12345".into(),
                    public_key: runtime_key.public_key,
                    public_key_fingerprint: String::new(),
                    binding_id: String::new(),
                    authentication_mode: WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity,
                    created_at: "2026-10-24T00:00:00Z".into(),
                    updated_at: "2026-10-24T00:00:00Z".into(),
                    revoked_at: None,
                },
                false,
            )
            .unwrap();
        store
    }

    fn request() -> WorkerSpawnRequest {
        WorkerSpawnRequest {
            intent: WorkerSpawnIntent::WorkspaceCoding,
            requested_worker_name: Some("Fixture Worker".into()),
            singleton_key: None,
            acceptance: WorkerSpawnAcceptanceRequirement::SocketReady,
            profile: ProfileSelector::Builtin("builtin:coder".into()),
            ticket_assignment: None,
            initial_submit: Vec::new(),
            workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachment_requests: Vec::new(),
            resolved_workdir_attachments: Vec::new(),
            resolved_config_bundle: None,
            resolved_workspace_api: None,
            resolved_memory_settings: None,
            resolved_subjektiv_attached: false,
            resolved_worker_observation_enabled: false,
            resolved_worker_observation_grants: Vec::new(),
            resolved_control_operation: None,
            resolved_execution_grant: None,
        }
    }

    fn seed(request: &WorkerSpawnRequest) -> FixedWorkerSeed<'_> {
        FixedWorkerSeed {
            workspace_id: WORKSPACE,
            runtime_id: RUNTIME,
            worker_id: WORKER.parse().unwrap(),
            allocation_key: "fixture-allocation",
            request,
        }
    }

    fn worker() -> RuntimeWorkerRef {
        RuntimeWorkerRef::new(RUNTIME, WORKER)
    }

    #[test]
    fn fixed_ids_are_isolated_and_seed_has_no_runtime_observation() {
        let a = bootstrap();
        let b = bootstrap();
        let request = request();
        let first = seed_fixed_worker(&a, seed(&request)).unwrap();
        let second = seed_fixed_worker(&b, seed(&request)).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.worker_id.to_string(), WORKER);
        assert_eq!(first.memory_settings.workspace_id, WORKSPACE);
        assert_eq!(first.memory_settings.language, "English");
        assert!(first.singleton.is_none());
        assert_ne!(
            a.get_workspace_signing_identity(WORKSPACE)
                .unwrap()
                .unwrap()
                .public_key,
            b.get_workspace_signing_identity(WORKSPACE)
                .unwrap()
                .unwrap()
                .public_key
        );
        assert_ne!(
            a.get_workspace_runtime_binding(WORKSPACE, RUNTIME)
                .unwrap()
                .unwrap()
                .public_key,
            b.get_workspace_runtime_binding(WORKSPACE, RUNTIME)
                .unwrap()
                .unwrap()
                .public_key
        );
        for store in [&a, &b] {
            assert_eq!(
                store
                    .worker_resource_key(WORKSPACE, &worker())
                    .unwrap()
                    .as_deref(),
                Some("W-1")
            );
            assert_eq!(
                store
                    .resolve_worker_resource_reference(WORKSPACE, "W-1")
                    .unwrap(),
                Some(worker())
            );
            assert!(
                !store
                    .has_active_worker_create_reservation(WORKSPACE, &worker())
                    .unwrap()
            );
            let registry = store
                .get_worker_registry(WORKSPACE, &worker())
                .unwrap()
                .unwrap();
            assert_eq!(registry.profile.as_deref(), Some("builtin:coder"));
            assert_eq!(registry.display_name, "Fixture Worker");
            assert_eq!(registry.retention_state, "normal");
            let projection = store
                .with_conn(|conn| read_worker_registry_projection(conn, WORKSPACE, &worker()))
                .unwrap()
                .unwrap();
            assert!(projection.observation.is_none());
            assert!(projection.job.is_none());
        }
    }

    #[test]
    fn initial_submit_is_refused_without_registration_or_allocation() {
        let store = bootstrap();
        let mut request = request();
        // Even an empty text segment is new input; the vector itself must be empty.
        for text in ["new input must not be seeded", ""] {
            request.initial_submit = vec![protocol::Segment::text(text)];
            let error = seed_fixed_worker(&store, seed(&request)).unwrap_err();
            assert!(
                matches!(error, Error::InvalidInput(ref message) if message.contains("initial_submit"))
            );
            assert!(
                store
                    .get_worker_registry(WORKSPACE, &worker())
                    .unwrap()
                    .is_none()
            );
            assert!(
                store
                    .worker_resource_key(WORKSPACE, &worker())
                    .unwrap()
                    .is_none()
            );
            assert!(
                !store
                    .has_active_worker_create_reservation(WORKSPACE, &worker())
                    .unwrap()
            );
        }
        // The rejected attempts must not consume the allocation or resource counter.
        request.initial_submit.clear();
        seed_fixed_worker(&store, seed(&request)).unwrap();
        assert_eq!(
            store
                .worker_resource_key(WORKSPACE, &worker())
                .unwrap()
                .as_deref(),
            Some("W-1")
        );
    }

    #[test]
    fn exact_replay_changed_input_and_cross_runtime_identity_are_refused() {
        let store = bootstrap();
        let mut request = request();
        seed_fixed_worker(&store, seed(&request)).unwrap();
        let before = store.get_worker_registry(WORKSPACE, &worker()).unwrap();
        assert!(seed_fixed_worker(&store, seed(&request)).is_err());
        request.requested_worker_name = Some("Must not overwrite".into());
        assert!(seed_fixed_worker(&store, seed(&request)).is_err());
        let mut new_allocation = seed(&request);
        new_allocation.allocation_key = "another-allocation";
        assert!(seed_fixed_worker(&store, new_allocation).is_err());
        let mut same_allocation = seed(&request);
        same_allocation.worker_id = WorkerId::now_v7();
        assert!(seed_fixed_worker(&store, same_allocation).is_err());
        assert_eq!(
            store.get_worker_registry(WORKSPACE, &worker()).unwrap(),
            before
        );

        let other = bootstrap();
        let mut record = before.unwrap();
        record.worker.runtime_id = "different-runtime".into();
        other.upsert_worker_registry(&record).unwrap();
        assert!(seed_fixed_worker(&other, seed(&request)).is_err());
        assert!(
            other
                .get_worker_registry(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reserved_and_orphaned_resource_identities_are_not_adopted() {
        let store = bootstrap();
        let request = request();
        let memory = store.get_workspace_memory_settings(WORKSPACE).unwrap();
        let ordinary = store
            .reserve_worker_create(WORKSPACE, RUNTIME, "ordinary", "fingerprint", None, &memory)
            .unwrap();
        let mut collision = seed(&request);
        collision.worker_id = ordinary.worker_id;
        assert!(seed_fixed_worker(&store, collision).is_err());
        // Store-level fault setup only: the external harness never writes SQL.
        store
            .with_conn(|conn| {
                allocate_resource_key(
                    conn,
                    WORKSPACE,
                    WorkspaceResourceKind::Worker,
                    &worker_resource_id(&worker()),
                    "now",
                )?;
                Ok(())
            })
            .unwrap();
        assert!(seed_fixed_worker(&store, seed(&request)).is_err());
        assert!(
            store
                .get_worker_registry(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_resource_identity_and_retained_removal_fence_are_refused() {
        let request = request();
        let malformed = bootstrap();
        malformed
            .with_conn(|conn| {
                allocate_resource_key(
                    conn,
                    WORKSPACE,
                    WorkspaceResourceKind::Worker,
                    "unknown-identity",
                    "now",
                )?;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            seed_fixed_worker(&malformed, seed(&request)),
            Err(Error::InvalidInput(_))
        ));
        assert!(
            malformed
                .get_worker_registry(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );

        let removed = bootstrap();
        removed.with_conn(|conn| {
            conn.execute(
                "INSERT INTO worker_registry_projection_removals(workspace_id, runtime_id, worker_id) VALUES (?1, ?2, ?3)",
                params![WORKSPACE, RUNTIME, WORKER],
            )?;
            Ok(())
        }).unwrap();
        assert!(matches!(
            seed_fixed_worker(&removed, seed(&request)),
            Err(Error::InvalidInput(_))
        ));
        assert!(
            removed
                .worker_resource_key(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_workspace_runtime_and_revoked_binding_are_refused() {
        let store = bootstrap();
        let request = request();
        let mut unknown = seed(&request);
        unknown.workspace_id = "unknown";
        assert!(seed_fixed_worker(&store, unknown).is_err());
        let mut unknown = seed(&request);
        unknown.runtime_id = "unknown";
        assert!(seed_fixed_worker(&store, unknown).is_err());
        store
            .revoke_workspace_runtime_binding(WORKSPACE, RUNTIME, "2026-10-24T01:00:00Z")
            .unwrap();
        assert!(seed_fixed_worker(&store, seed(&request)).is_err());
        assert!(
            store
                .worker_resource_key(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unsupported_intent_is_refused_without_partial_registration() {
        let store = bootstrap();
        let base = request();
        let mut requests = Vec::new();
        let mut request = base.clone();
        request.singleton_key = Some("fixture-singleton".into());
        requests.push(request);
        let mut request = base.clone();
        request.intent = WorkerSpawnIntent::WorkspaceOrchestrator;
        requests.push(request);
        let mut request = base.clone();
        request.resolved_subjektiv_attached = true;
        requests.push(request);
        let mut request = base.clone();
        request
            .workdir_attachment_requests
            .push(WorkerSpawnWorkingDirectoryAttachmentRequest {
                alias: workdir::WorkdirAttachmentAlias::new("repo").unwrap(),
                working_directory: WorkerSpawnWorkingDirectoryRequest {
                    repository_key: "repo".into(),
                    selector: None,
                },
            });
        requests.push(request);
        for request in requests {
            assert!(seed_fixed_worker(&store, seed(&request)).is_err());
        }
        assert!(
            store
                .get_worker_registry(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .worker_resource_key(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            seed_fixed_worker(&store, seed(&base))
                .unwrap()
                .worker_id
                .to_string(),
            WORKER
        );
    }

    #[test]
    fn late_catalog_failure_rolls_back_reservation_and_resource_allocation() {
        let store = bootstrap();
        let request = request();
        store
            .with_conn(|conn| {
                conn.execute_batch(
                    "CREATE TEMP TRIGGER fail_fixture_catalog BEFORE INSERT ON worker_registry
                                BEGIN SELECT RAISE(ABORT, 'injected catalog failure'); END",
                )?;
                Ok(())
            })
            .unwrap();
        assert!(seed_fixed_worker(&store, seed(&request)).is_err());
        assert!(
            !store
                .has_active_worker_create_reservation(WORKSPACE, &worker())
                .unwrap()
        );
        assert!(
            store
                .worker_resource_key(WORKSPACE, &worker())
                .unwrap()
                .is_none()
        );
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER fail_fixture_catalog")?;
                Ok(())
            })
            .unwrap();
        seed_fixed_worker(&store, seed(&request)).unwrap();
        assert_eq!(
            store
                .worker_resource_key(WORKSPACE, &worker())
                .unwrap()
                .as_deref(),
            Some("W-1")
        );
    }

    #[test]
    fn ordinary_creation_still_allocates_and_replays_without_catalog_insertion() {
        let store = bootstrap();
        let memory = store.get_workspace_memory_settings(WORKSPACE).unwrap();
        let reserve = |key| {
            store
                .reserve_worker_create(WORKSPACE, RUNTIME, key, "fingerprint", None, &memory)
                .unwrap()
        };
        let first = reserve("ordinary-one");
        assert_eq!(first, reserve("ordinary-one"));
        let second = reserve("ordinary-two");
        assert_ne!(first.worker_id, second.worker_id);
        assert_ne!(first.worker_id.to_string(), WORKER);
        assert_eq!(
            first.worker_id.as_uuid().get_version(),
            Some(uuid::Version::SortRand)
        );
        let worker = RuntimeWorkerRef::new(RUNTIME, first.worker_id.to_string());
        assert!(
            store
                .has_active_worker_create_reservation(WORKSPACE, &worker)
                .unwrap()
        );
        assert!(
            store
                .get_worker_registry(WORKSPACE, &worker)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .reserve_worker_create(WORKSPACE, RUNTIME, "ordinary-one", "changed", None, &memory)
                .is_err()
        );
    }
}
