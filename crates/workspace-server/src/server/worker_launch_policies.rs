//! Owner-only launch policy management. Usage keys are never administration grants.
//!
//! The DB executor owns registry exclusion, the IMMEDIATE transaction and local
//! publication through completion, even if the HTTP waiter is cancelled. Replay
//! returns the Store's historical receipt without resolving today's Profiles.

use super::*;
use server_api::launch_policy::{
    WorkerLaunchPoliciesApi, WorkerLaunchPoliciesResponse, WorkerLaunchPolicyAction,
    WorkerLaunchPolicyMutation, WorkerLaunchPolicyReceipt,
};
use server_api::{RepositoryApiError, ServerRequestContext};
use worker::feature::permissions::FeaturePermissionRegistry;

fn administration_account(context: &ServerRequestContext) -> Result<&str> {
    // Reject both forms, including a mixed context carrying an owner actor. A
    // Runtime/Worker's policy-use permission must never become owner authority.
    if context.runtime_source.is_some() || context.worker_source.is_some() {
        return Err(Error::WorkspacePermissionDenied(
            "Launch policy administration is not a Runtime or Worker capability".into(),
        ));
    }
    context
        .actor
        .as_ref()
        .map(|actor| actor.account_id.as_str())
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied(
                "Launch policy administration requires an authenticated owner".into(),
            )
        })
}

impl WorkerLaunchPoliciesApi for ServerApiContractService {
    async fn worker_launch_policies(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
    ) -> std::result::Result<WorkerLaunchPoliciesResponse, RepositoryApiError> {
        let api = self.workspace_api()?.clone();
        api.store
            .db_executor()
            .run(move || {
                validate_workspace_scope(&api, &workspace_id)?;
                let account = administration_account(&context)?;
                let policies = api.config_store.with_conn(|connection| {
                    // Owner and rows must come from the same database snapshot.
                    let tx = connection.unchecked_transaction()?;
                    crate::launch_policy::require_owner(&tx, &workspace_id, account)?;
                    crate::launch_policy::policies(&tx, &workspace_id)
                })?;
                Ok::<_, ApiError>(WorkerLaunchPoliciesResponse { policies })
            })
            .await
            .map_err(ApiError::into_repository_api_error)
    }

    async fn mutate_worker_launch_policy(
        &self,
        context: ServerRequestContext,
        workspace_id: String,
        request: WorkerLaunchPolicyMutation,
    ) -> std::result::Result<WorkerLaunchPolicyReceipt, RepositoryApiError> {
        let api = self.workspace_api()?.clone();
        api.store
            .db_executor()
            .run(move || {
                validate_workspace_scope(&api, &workspace_id)?;
                let account = administration_account(&context)?;
                // Same order as live permission refresh/effect admission:
                // registry -> Workspace DB. Never call a Store getter which
                // reacquires the connection from inside this transaction.
                let mut registry = api.permission_registry.write().map_err(|_| {
                    Error::Config("current Feature declarations unavailable".into())
                })?;
                let root = ServerConfig::server_data_root_for_data_dir(&api.config.data_root)
                    .join("plugin-artifacts");
                api.config_store
                    .with_conn_mut(|connection| {
                        mutate_and_publish(
                            connection,
                            &workspace_id,
                            account,
                            &request,
                            &mut registry,
                            |transaction, candidate| {
                                // This also projects launch keys through the worker
                                // Feature's existing namespace handle. It sees the
                                // uncommitted new row, allowing explicit self-use.
                                api.config_store
                                    .sync_plugin_permission_registry_in_connection(
                                        transaction,
                                        &root,
                                        &workspace_id,
                                        candidate,
                                    )?;
                                validate_management_input(
                                    transaction,
                                    &workspace_id,
                                    candidate,
                                    &request.action,
                                )
                            },
                        )
                    })
                    .map_err(ApiError::from)
            })
            .await
            .map_err(ApiError::into_repository_api_error)
    }
}

/// Keep the connection owner and registry writer through commit AND publication.
/// Preparation receives only the transaction's connection and an unpublished
/// clone. Returning an error drops every policy/history/receipt write and leaves
/// the live catalog untouched. No Runtime I/O or separate database is involved.
fn mutate_and_publish(
    connection: &mut rusqlite::Connection,
    workspace: &str,
    account: &str,
    request: &WorkerLaunchPolicyMutation,
    registry: &mut FeaturePermissionRegistry,
    prepare: impl FnOnce(&rusqlite::Connection, &mut FeaturePermissionRegistry) -> Result<()>,
) -> Result<WorkerLaunchPolicyReceipt> {
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    crate::launch_policy::require_owner(&tx, workspace, account)?;
    if registry.host().catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    // This is only a branch discriminator, not replay authorization. The Store
    // below still checks current owner, syntax and exact full input + actor before
    // returning any receipt. Never compare the current policy to a saved receipt.
    let replay: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM worker_launch_policy_operations WHERE workspace_id=?1 AND operation_key=?2)",
        rusqlite::params![workspace, request.operation_key],
        |row| row.get(0),
    )?;
    let receipt =
        crate::launch_policy::mutate(&tx, workspace, account, request).map_err(mutation_error)?;
    if replay {
        tx.commit()?;
        return Ok(receipt);
    }
    let mut candidate = registry.clone();
    prepare(&tx, &mut candidate)?;
    tx.commit()?;
    // Infallible local publication, still under both ownership guards. A process
    // restart rebuilds declarations from committed DB rows, not this receipt.
    *registry = candidate;
    Ok(receipt)
}

fn mutation_error(error: Error) -> Error {
    match error {
        // Store currently represents these domain conflicts as InvalidInput.
        // Project them as 409 without relabeling malformed input as a conflict.
        Error::InvalidInput(message)
            if message.starts_with("Launch policy CAS conflict;")
                || message
                    == "Launch policy operation key reused with different full input or actor" =>
        {
            Error::RepositoryConflict(message)
        }
        other => other,
    }
}

fn validate_management_input(
    connection: &rusqlite::Connection,
    workspace: &str,
    registry: &FeaturePermissionRegistry,
    action: &WorkerLaunchPolicyAction,
) -> Result<()> {
    let (allowed_profiles, child_permissions) = match action {
        WorkerLaunchPolicyAction::Create {
            allowed_profiles,
            child_permissions,
            ..
        }
        | WorkerLaunchPolicyAction::ReplaceAuthorization {
            allowed_profiles,
            child_permissions,
        } => (allowed_profiles, child_permissions),
        // A removed Profile or permission reference must not prevent an owner
        // from renaming, revoking or deleting its old policy.
        _ => return Ok(()),
    };
    let state = crate::config_source::load_state(connection, workspace)?.ok_or_else(|| {
        Error::RegistryInconsistency("Workspace Profile configuration is unavailable".into())
    })?;
    let evaluation = crate::config_source::evaluate_workspace_config_state(
        &state,
        state.contract.schema_bundle.clone(),
    )?;
    let profiles =
        crate::profile_settings::project_profiles_from_evaluation(workspace, &state, &evaluation)?;
    validate_registered_profiles(&profiles, allowed_profiles)?;
    // Reuse the existing Set/Namespace resolver, including exclusions and the
    // validation of references hidden by revoked=true. Do not persist expansion
    // or enforce subset-of-owner execution permissions on an owner-managed policy.
    let source = crate::policy_config::source(&evaluation)?;
    let host = crate::policy_config::configured_host(registry.host(), &source)?;
    crate::policy_config::validate_assignment(
        &host,
        &crate::policy_config::AssignmentSource::from_assignment(child_permissions),
    )
}

fn validate_registered_profiles(
    projection: &crate::profile_settings::ProfileConfigProjection,
    profiles: &[String],
) -> Result<()> {
    for profile in profiles {
        server_api::launch_policy::validate_canonical_profile(profile)
            .map_err(Error::InvalidInput)?;
        let selected =
            crate::profile_settings::selector_for_registered_profile(projection, profile);
        let exact = match selected {
            Some(ProfileSelector::Builtin(key) | ProfileSelector::Named(key)) => key == *profile,
            None => false,
        };
        if !exact {
            return Err(Error::InvalidInput(format!(
                "Launch policy Profile `{profile}` is not an exact registered selector"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Component tests of management's actual transaction/publication boundary.
    //! The Workspace/config fixture is minimal; these are not HTTP authentication
    //! or Runtime-dispatch tests.
    use super::*;
    use feature_permissions::{
        NamespaceId, NamespacePermissions, PermissionAssignment, PermissionKey,
        PermissionReference, PermissionSetId, WorkspaceId,
    };

    fn context() -> ServerRequestContext {
        ServerRequestContext {
            actor: Some(RequestActor {
                user_id: "human".into(),
                account_id: "owner".into(),
                handle: "owner".into(),
                display_name: "Owner".into(),
                auth_method: server_api::ActorAuthMethod::ApiToken,
            }),
            worker_source: None,
            runtime_source: None,
            origin: None,
            transport_headers: vec![],
        }
    }

    fn fixture() -> (rusqlite::Connection, FeaturePermissionRegistry) {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE workspaces (workspace_id TEXT PRIMARY KEY,
                     owner_account_id TEXT NOT NULL, state TEXT NOT NULL);
                 INSERT INTO workspaces VALUES ('w','owner','active');",
            )
            .unwrap();
        connection
            .execute_batch(crate::launch_policy::CURRENT_SCHEMA)
            .unwrap();
        let registry = worker::feature::permissions::builtin_permission_registry(
            WorkspaceId::parse("w").unwrap(),
        )
        .unwrap();
        (connection, registry)
    }

    fn create() -> WorkerLaunchPolicyMutation {
        WorkerLaunchPolicyMutation {
            operation_key: "create".into(),
            policy_id: "coder".into(),
            expected: None,
            action: WorkerLaunchPolicyAction::Create {
                name: "Coder".into(),
                allowed_profiles: vec!["builtin:coder".into()],
                child_permissions: PermissionAssignment::default(),
            },
        }
    }

    fn sync(
        connection: &rusqlite::Connection,
        registry: &mut FeaturePermissionRegistry,
    ) -> Result<()> {
        sync_launch_policy_permissions(connection, "w", registry)
    }

    #[test]
    fn owner_actor_is_not_inferred_from_usage_or_mixed_runtime_context() {
        let mut request = context();
        assert_eq!(administration_account(&request).unwrap(), "owner");
        request.runtime_source = Some(server_api::ServerRuntimeSource {
            runtime_id: "r".into(),
            worker_id: None,
            authentication_binding_id: "binding".into(),
        });
        assert!(administration_account(&request).is_err());
        request.runtime_source = None;
        request.worker_source = Some(server_api::ServerWorkerSource {
            runtime_id: "r".into(),
            worker_id: "worker".into(),
        });
        assert!(administration_account(&request).is_err());
        request.worker_source = None;
        request.actor = None;
        assert!(administration_account(&request).is_err());
    }

    #[test]
    fn management_owner_and_workspace_are_checked_even_for_replay() {
        let (mut connection, mut registry) = fixture();
        let request = create();
        mutate_and_publish(&mut connection, "w", "owner", &request, &mut registry, sync).unwrap();
        for (workspace, account) in [("w", "not-owner"), ("other", "owner")] {
            assert!(
                mutate_and_publish(
                    &mut connection,
                    workspace,
                    account,
                    &request,
                    &mut registry,
                    |_, _| panic!("unauthorized request reached preparation"),
                )
                .is_err()
            );
        }
        connection
            .execute("UPDATE workspaces SET state='deleted'", [])
            .unwrap();
        assert!(
            mutate_and_publish(
                &mut connection,
                "w",
                "owner",
                &request,
                &mut registry,
                |_, _| panic!("inactive Workspace reached preparation"),
            )
            .is_err()
        );
    }

    #[test]
    fn failed_validation_rolls_back_rows_receipts_and_candidate_catalog() {
        let (mut connection, mut registry) = fixture();
        let before = registry.host().catalog();
        let failure = mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &create(),
            &mut registry,
            |transaction, candidate| {
                sync(transaction, candidate)?;
                assert_ne!(candidate.host().catalog(), before);
                Err(Error::InvalidInput(
                    "Profile removed or assignment invalid".into(),
                ))
            },
        );
        assert!(failure.is_err());
        assert_eq!(registry.host().catalog(), before);
        assert!(
            crate::launch_policy::policies(&connection, "w")
                .unwrap()
                .is_empty()
        );
        for table in [
            "worker_launch_policy_history",
            "worker_launch_policy_operations",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
        }
        // The same key remains usable after a known transaction rollback.
        mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &create(),
            &mut registry,
            sync,
        )
        .unwrap();
    }

    #[test]
    fn replay_skips_current_resolution_and_never_republishes_old_authorization() {
        let (mut connection, mut registry) = fixture();
        let original = create();
        let receipt = mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &original,
            &mut registry,
            sync,
        )
        .unwrap();
        let revoke = WorkerLaunchPolicyMutation {
            operation_key: "revoke".into(),
            policy_id: original.policy_id.clone(),
            expected: Some(receipt.policy.observation()),
            action: WorkerLaunchPolicyAction::Revoke,
        };
        let revoked =
            mutate_and_publish(&mut connection, "w", "owner", &revoke, &mut registry, sync)
                .unwrap();
        let catalog = registry.host().catalog();
        let replay = mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &original,
            &mut registry,
            |_, _| {
                panic!("replay must not resolve changed/deleted Profiles or permission references")
            },
        )
        .unwrap();
        assert_eq!(replay, receipt);
        assert_eq!(registry.host().catalog(), catalog);
        assert_eq!(
            crate::launch_policy::policy(&connection, "w", "coder")
                .unwrap()
                .unwrap(),
            revoked.policy
        );
        let mut changed = original.clone();
        changed.action = WorkerLaunchPolicyAction::Create {
            name: "Different full input".into(),
            allowed_profiles: vec!["builtin:coder".into()],
            child_permissions: PermissionAssignment::default(),
        };
        assert!(matches!(
            mutate_and_publish(
                &mut connection,
                "w",
                "owner",
                &changed,
                &mut registry,
                |_, _| panic!("mismatched replay reached preparation"),
            ),
            Err(Error::RepositoryConflict(_))
        ));
        let mut stale = revoke;
        stale.operation_key = "stale".into();
        assert!(matches!(
            mutate_and_publish(&mut connection, "w", "owner", &stale, &mut registry, sync),
            Err(Error::RepositoryConflict(_))
        ));
    }

    #[test]
    fn commit_failure_does_not_publish_candidate_catalog() {
        let (mut connection, mut registry) = fixture();
        connection
            .execute_batch(
                "CREATE TABLE commit_failure (
                     workspace TEXT REFERENCES workspaces(workspace_id)
                     DEFERRABLE INITIALLY DEFERRED);",
            )
            .unwrap();
        let before = registry.host().catalog();
        let result = mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &create(),
            &mut registry,
            |transaction, candidate| {
                sync(transaction, candidate)?;
                // Fails at commit, after every validation/preparation succeeds.
                transaction.execute("INSERT INTO commit_failure VALUES ('absent')", [])?;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(registry.host().catalog(), before);
        assert!(
            crate::launch_policy::policies(&connection, "w")
                .unwrap()
                .is_empty()
        );
        mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &create(),
            &mut registry,
            sync,
        )
        .unwrap();
    }

    fn profile_projection(source: &str) -> crate::profile_settings::ProfileConfigProjection {
        use crate::config_source::WorkspaceConfigSchemaProvider;
        let schema = config_source::WorkspaceConfigSchemaBundle::compose([
            crate::profile_settings::ProfileConfigSchemaProvider
                .contribution()
                .unwrap(),
            crate::prompt_settings::PromptConfigSchemaProvider
                .contribution()
                .unwrap(),
        ])
        .unwrap();
        let snapshot = config_source::ConfigTreeSnapshot::from_entries(vec![
            config_source::ConfigEntry::new(
                config_source::VirtualPath::parse("main.dcdl").unwrap(),
                config_source::ConfigContentType::Decodal,
                source,
            )
            .unwrap(),
        ])
        .unwrap();
        let contract = config_source::ToolchainContract::with_schema_bundle(
            config_source::DEFAULT_SCHEMA_VERSION,
            vec![config_source::VirtualPath::parse("main.dcdl").unwrap()],
            config_source::DEFAULT_IMPORT_POLICY_VERSION,
            schema,
        );
        let projection_digest = config_source::SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&contract)
            .unwrap()
            .projection_digest;
        let state = crate::config_source::WorkspaceConfigState {
            snapshot,
            contract,
            projection_digest,
        };
        crate::profile_settings::project_profiles_from_workspace_config("w", &state).unwrap()
    }

    #[test]
    fn profiles_require_exact_registry_source_and_never_default_or_slug_fallback() {
        let projection = profile_projection(
            r#"{ profile = { entries = [{ selector = "project:coder";
                profile = { slug = "coder"; model = { id = "gpt-5.4"; }; };
            }]; }; }"#,
        );
        for profile in ["builtin:coder", "project:coder", "builtin:default"] {
            validate_registered_profiles(&projection, &[profile.into()]).unwrap();
        }
        for invalid in [
            "coder",
            "default",
            "inherit",
            "builtin:missing",
            "project:missing",
            "project:../coder",
        ] {
            assert!(validate_registered_profiles(&projection, &[invalid.into()]).is_err());
        }
        let removed = profile_projection("{}");
        assert!(validate_registered_profiles(&removed, &["project:coder".into()]).is_err());
        // The same builtin name does not rescue the removed project selector.
        validate_registered_profiles(&removed, &["builtin:coder".into()]).unwrap();
    }

    #[test]
    fn rename_revoke_and_delete_do_not_need_current_profile_configuration() {
        let (connection, registry) = fixture();
        // This fixture has no config tables: any accidental resolution fails.
        for action in [
            WorkerLaunchPolicyAction::Rename {
                name: "Renamed".into(),
            },
            WorkerLaunchPolicyAction::Revoke,
            WorkerLaunchPolicyAction::Delete,
        ] {
            validate_management_input(&connection, "w", &registry, &action).unwrap();
        }
    }

    #[test]
    fn assignment_validation_uses_existing_resolver_for_self_use_and_hidden_errors() {
        let (mut connection, mut registry) = fixture();
        mutate_and_publish(
            &mut connection,
            "w",
            "owner",
            &create(),
            &mut registry,
            |transaction, candidate| {
                sync(transaction, candidate)?;
                let assignment = PermissionAssignment {
                    grants: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse("worker").unwrap(),
                        keys: vec![PermissionKey::parse("worker.launch.coder").unwrap()],
                    })],
                    ..Default::default()
                };
                crate::policy_config::validate_assignment(
                    candidate.host(),
                    &crate::policy_config::AssignmentSource::from_assignment(&assignment),
                )?;
                for assignment in [
                    PermissionAssignment {
                        grants: vec![PermissionReference::Set(
                            PermissionSetId::parse("missing").unwrap(),
                        )],
                        revoked: true,
                        ..Default::default()
                    },
                    PermissionAssignment {
                        exclusions: vec![PermissionReference::Namespace(NamespacePermissions {
                            namespace: NamespaceId::parse("worker").unwrap(),
                            keys: vec![PermissionKey::parse("worker.launch.unknown").unwrap()],
                        })],
                        ..Default::default()
                    },
                ] {
                    assert!(
                        crate::policy_config::validate_assignment(
                            candidate.host(),
                            &crate::policy_config::AssignmentSource::from_assignment(&assignment),
                        )
                        .is_err()
                    );
                }
                Ok(())
            },
        )
        .unwrap();
    }
}
