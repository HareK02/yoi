//! Creation authority is selected by authenticated caller kind, never by Profile.
use super::*;
use server_api::authorization::{
    CoreWorkerExecutionGrant, CoreWorkerGrantSource, PermissionReferenceEncoding,
};
use server_api::launch_policy::WorkerCreationAuthorization;

pub(super) fn resolve_core_creation_grant(
    api: &WorkspaceApi,
    source: Option<&RuntimeWorkerRef>,
    account: Option<&str>,
    authorization: &WorkerCreationAuthorization,
    runtime: &str,
    profile: &str,
    bundle: &worker_runtime::config_bundle::ConfigBundle,
    operation: &WorkerControlOperation,
) -> ApiResult<CoreWorkerExecutionGrant> {
    let digest = bundle.computed_digest();
    match (source, authorization) {
        (
            Some(source),
            WorkerCreationAuthorization::Policy {
                policy_id,
                policy_revision,
            },
        ) => subject_authorization::prepare_core_policy_grant(
            api,
            source,
            policy_id,
            *policy_revision,
            runtime,
            profile,
            &digest,
            &operation.operation_id,
        ),
        (Some(_), WorkerCreationAuthorization::Human { .. }) => {
            Err(Error::WorkspacePermissionDenied(
                "Worker-origin creation cannot use human initial permissions or defaults".into(),
            )
            .into())
        }
        (None, WorkerCreationAuthorization::Policy { .. }) => {
            Err(Error::WorkspacePermissionDenied(
                "policy use requires an authenticated Worker origin".into(),
            )
            .into())
        }
        (None, WorkerCreationAuthorization::Human { assignment }) => {
            let account = account.ok_or_else(|| {
                Error::WorkspacePermissionDenied(
                    "human initial grant requires an authenticated Workspace owner".into(),
                )
            })?;
            api.config_store.with_conn(|connection| {
                crate::launch_policy::require_owner(connection, api.workspace_id(), account)
            })?;
            api.refresh_plugin_permission_registry()?;
            let registry = api
                .permission_registry
                .read()
                .map_err(|_| Error::Config("Feature declarations unavailable".into()))?;
            let assignment = api.config_store.human_worker_assignment(
                api.workspace_id(),
                registry.host(),
                assignment.as_ref(),
            )?;
            Ok(CoreWorkerExecutionGrant {
                grant_id: new_id("weg"),
                workspace_id: api.workspace_id().into(),
                creation_operation_id: operation.operation_id.clone(),
                runtime_id: runtime.into(),
                worker_id: String::new(),
                profile: profile.into(),
                profile_digest: digest,
                assignment,
                reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
                issuance_ceiling: None,
                source: CoreWorkerGrantSource::Human {
                    account_id: account.into(),
                },
            })
        }
    }
}

/// Shared public entrypoint resolution. Caller holds the operation lock; replays
/// use only the immutable saved bundle, never current Profile/default resolution.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_creation_profile(
    api: &WorkspaceApi,
    source: Option<&RuntimeWorkerRef>,
    account: Option<&str>,
    authorization: &WorkerCreationAuthorization,
    runtime_id: &str,
    profile: Option<String>,
    resolved_control_operation: Option<&WorkerControlOperation>,
    subject_connection: bool,
) -> ApiResult<(
    String,
    ProfileSelector,
    Option<worker_runtime::config_bundle::ConfigBundle>,
    Option<CoreWorkerExecutionGrant>,
)> {
    match (source, authorization) {
        (None, WorkerCreationAuthorization::Human { .. }) => {
            let account = account.ok_or_else(|| {
                Error::WorkspacePermissionDenied(
                    "Worker creation requires authenticated owner context".into(),
                )
            })?;
            api.config_store.with_conn(|conn| {
                crate::launch_policy::require_owner(conn, api.workspace_id(), account)
            })?;
        }
        (Some(_), WorkerCreationAuthorization::Policy { .. }) => {}
        _ => {
            return Err(Error::WorkspacePermissionDenied(
                "creation authorization does not match authenticated caller kind".into(),
            )
            .into());
        }
    }
    let saved = resolved_control_operation
        .as_ref()
        .map(|operation| {
            api.config_store.saved_core_worker_launch(
                api.workspace_id(),
                &operation.operation_id,
                &operation.input_fingerprint,
            )
        })
        .transpose()?
        .flatten();
    let (profile, profile_selector, resolved_config_bundle, resolved_core_execution_grant) =
        if let Some((grant, bundle)) = saved {
            subject_authorization::revalidate_core_grant(&api, &grant)?;
            let selector = if grant.profile.starts_with("builtin:") {
                ProfileSelector::Builtin(grant.profile.clone())
            } else if grant.profile.starts_with("project:") {
                ProfileSelector::Named(grant.profile.clone())
            } else {
                return Err(Error::Config("saved grant has noncanonical Profile".into()).into());
            };
            (grant.profile.clone(), selector, Some(bundle), Some(grant))
        } else {
            let config_state = api
                .config_store
                .load_workspace_config(&api.config.workspace_id)?
                .ok_or_else(|| Error::InvalidRecordId("virtual config source tree".into()))?;
            let profile_projection =
                crate::profile_settings::project_profiles_from_workspace_config(
                    &api.config.workspace_id,
                    &config_state,
                )?;
            let profile = if source.is_some() {
                let profile = profile.as_deref().ok_or_else(|| {
                    Error::InvalidInput(
                        "policy launch requires an explicit canonical Profile selector".into(),
                    )
                })?;
                server_api::launch_policy::validate_canonical_profile(profile)
                    .map_err(Error::InvalidInput)?;
                profile.to_owned()
            } else {
                profile
                    .filter(|profile| !profile.trim().is_empty())
                    .or_else(|| profile_projection.settings.default_profile.clone())
                    .ok_or_else(|| {
                        settings_bad_request(
                            "workspace_default_profile_missing",
                            "profile is required because this Workspace has no default profile",
                        )
                    })?
            };
            let profile_selector = if source.is_some() {
                crate::profile_settings::selector_for_registered_profile(
                    &profile_projection,
                    &profile,
                )
            } else {
                crate::profile_settings::selector_for_workspace_candidate(
                    &profile_projection,
                    &profile,
                )
            }
            .ok_or_else(|| {
                settings_bad_request(
                    "unsupported_worker_profile",
                    "profile must be an exact Backend-published profile candidate",
                )
            })?;
            let canonical_profile = match &profile_selector {
                ProfileSelector::Builtin(name) | ProfileSelector::Named(name) => name.clone(),
            };
            if source.is_some() && profile != canonical_profile {
                return Err(Error::InvalidInput(
                    "policy Profile must be canonical and source-qualified".into(),
                )
                .into());
            }
            let prompt_catalog = api
                .prompt_projection_cache
                .resolve(&api.config.workspace_id, &config_state)?;
            let bundle = crate::profile_settings::build_virtual_profile_config_bundle_with_prompt_projection(&profile_projection, &config_state, &api.config.workspace_id, &api.config.workspace_created_at, &profile, prompt_catalog.as_ref())?.ok_or_else(|| Error::Config("resolved Worker Profile bundle is missing".into()))?;
            let grant = if subject_connection {
                None
            } else {
                Some(core_worker_creation::resolve_core_creation_grant(
                    &api,
                    source,
                    account,
                    &authorization,
                    &runtime_id,
                    &canonical_profile,
                    &bundle,
                    resolved_control_operation
                        .as_ref()
                        .expect("ordinary creation operation checked"),
                )?)
            };
            (canonical_profile, profile_selector, Some(bundle), grant)
        };
    Ok((
        profile,
        profile_selector,
        resolved_config_bundle,
        resolved_core_execution_grant,
    ))
}

/// Directional control is a separate Feature record, repaired on exact replay
/// only after the child registry identity has committed.
pub(super) fn persist_spawn_control(
    api: &WorkspaceApi,
    controller: &RuntimeWorkerRef,
    child: &RuntimeWorkerRef,
    operation_key: &str,
    allocation_key: &str,
    assigned: bool,
) -> ApiResult<()> {
    subject_authorization::persist_reply_grant(api, controller, child, allocation_key)?;
    api.store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: api.workspace_id().into(),
            grant_id: new_id("wcg"),
            controller: controller.clone(),
            subject: child.clone(),
            relation: if assigned { "assigned" } else { "spawned" }.into(),
            origin: "worker_spawn".into(),
            permissions: [
                "send_input",
                "notify",
                "cancel",
                "stop",
                "restore",
                "remove",
                "observe",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            operation_id: operation_key.into(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            revoked_at: None,
        })?;
    Ok(())
}
