use super::*;
use server_api::subject_policy::{
    SubjectPermissionPolicyMutation, SubjectPermissionPolicyMutationResponse,
    SubjectPermissionPolicyResponse,
};

pub(super) async fn get(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace_id: String,
    subject_id: String,
) -> ApiResult<SubjectPermissionPolicyResponse> {
    let actor = subject_authorization::administration_actor(&context)?;
    validate_workspace_scope(&api, &workspace_id)?;
    let workspace = api
        .store
        .get_workspace(&workspace_id)
        .await?
        .ok_or(Error::WorkspaceIdMismatch)?;
    let can_manage = workspace.owner_account_id == actor.account_id;
    api.store
        .db_executor()
        .run(move || -> ApiResult<_> {
            let native = open_subjektiv_store(&api)?;
            let identity = native.database_identity().map_err(subjektiv_store_error)?;
            native
                .with_permission_authority_guard(&identity, |authority| {
                    authority.require_subject(&subject_id)?;
                    Ok((|| -> ApiResult<_> {
                        let registry = api.permission_registry.read().map_err(|_| {
                            Error::Config("current Feature declarations unavailable".into())
                        })?;
                        // State identity, policy and shared-set overlay come from one
                        // domain snapshot while native lifecycle and catalog stay pinned.
                        let (change_id, policy, host) =
                            api.config_store.subject_permission_policy_view(
                                &workspace_id,
                                &subject_id,
                                registry.host(),
                            )?;
                        // These projections describe references, not effective grants:
                        // exact legacy domain constraints remain in the response policy.
                        let execution_resolution = host
                            .resolve(policy.as_ref().map(|policy| &policy.execution))
                            .projection()
                            .clone();
                        let delegation_resolution = host
                            .resolve(policy.as_ref().map(|policy| &policy.delegation))
                            .projection()
                            .clone();
                        Ok(SubjectPermissionPolicyResponse {
                            change_id,
                            policy,
                            can_manage,
                            catalog: host.catalog(),
                            execution_resolution,
                            delegation_resolution,
                        })
                    })())
                })
                .map_err(subjektiv_store_error)?
        })
        .await
}

pub(super) async fn set(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace_id: String,
    subject_id: String,
    request: SubjectPermissionPolicyMutation,
) -> ApiResult<SubjectPermissionPolicyMutationResponse> {
    let actor = subject_authorization::administration_actor(&context)?;
    require_workspace_owner(
        &api,
        &workspace_id,
        actor,
        "Subject permission administration",
    )
    .await?;
    let account_id = actor.account_id.clone();
    let mutation_api = api.clone();
    let mutation_subject = subject_id.clone();
    let (response, changed) = api
        .store
        .db_executor()
        .run(move || -> ApiResult<_> {
            let native = open_subjektiv_store(&mutation_api)?;
            let identity = native.database_identity().map_err(subjektiv_store_error)?;
            native
                .with_permission_authority_guard(&identity, |authority| {
                    authority.require_subject(&mutation_subject)?;
                    Ok((|| -> ApiResult<_> {
                        // Match creation admission: native lifecycle -> catalog -> Server.
                        let registry = mutation_api.permission_registry.read().map_err(|_| {
                            Error::Config("current Feature declarations unavailable".into())
                        })?;
                        let before = mutation_api
                            .config_store
                            .current_subject_permission_policy(&workspace_id, &mutation_subject)?;
                        let response = mutation_api.config_store.mutate_subject_permission_policy(
                            &workspace_id,
                            &account_id,
                            &mutation_subject,
                            &request,
                            registry.host(),
                        )?;
                        let after = mutation_api
                            .config_store
                            .current_subject_permission_policy(&workspace_id, &mutation_subject)?;
                        // A replay response may describe an old policy, not today's state.
                        Ok((response, before != after))
                    })())
                })
                .map_err(subjektiv_store_error)?
        })
        .await?;
    if changed {
        let _ = subject_authorization::interrupt_subject_workers(&api, &subject_id).await;
    }
    Ok(response)
}
