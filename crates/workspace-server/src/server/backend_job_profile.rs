//! Profile recipes are selected by the caller. Execution grants remain Host owned.
use super::*;

impl WorkspaceApi {
    pub(super) fn resolve_backend_job_profile(
        &self,
        request: &BackendJobRequest,
    ) -> Result<(ProfileSelector, worker_runtime::config_bundle::ConfigBundle)> {
        request.validate()?;
        let state = self
            .config_store
            .load_workspace_config(self.workspace_id())?
            .ok_or_else(|| {
                Error::Config("Backend Job requires Workspace Profile configuration".into())
            })?;
        let projection = crate::profile_settings::project_profiles_from_workspace_config(
            self.workspace_id(),
            &state,
        )?;
        let requested_profile = if request.profile == "default" {
            projection
                .settings
                .default_profile
                .as_deref()
                .ok_or_else(|| {
                    Error::InvalidInput("Workspace has no configured default Job Profile".into())
                })?
        } else {
            request.profile.as_str()
        };
        let selector = crate::profile_settings::selector_for_registered_profile(
            &projection,
            requested_profile,
        )
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "Backend Job Profile `{}` is not a published registry candidate",
                request.profile
            ))
        })?;
        let profile_key = match &selector {
            ProfileSelector::Builtin(key) | ProfileSelector::Named(key) => key.as_str(),
        };
        let prompts = self
            .prompt_projection_cache
            .resolve(self.workspace_id(), &state)?;
        let bundle =
            crate::profile_settings::build_virtual_profile_config_bundle_with_prompt_projection(
                &projection,
                &state,
                self.workspace_id(),
                &self.config.workspace_created_at,
                profile_key,
                prompts.as_ref(),
            )?
            .ok_or_else(|| Error::Config("Backend Job Profile bundle is missing".into()))?;
        let manifest = crate::profile_settings::resolve_profile_manifest_from_config_bundle(
            &bundle,
            profile_key,
        )?;
        validate_backend_job_profile_requirements(&manifest, request)?;
        Ok((selector, bundle))
    }
}

fn validate_backend_job_profile_requirements(
    manifest: &manifest::WorkerManifest,
    request: &BackendJobRequest,
) -> Result<()> {
    let f = &manifest.feature;
    // These tools require domain/parent/Workdir authority which a bounded Job
    // deliberately does not inherit. Reject, rather than disabling features or
    // substituting a different recipe under the caller's selector.
    if f.memory.enabled()
        || f.worker.enabled
        || f.sub_worker.enabled
        || f.flow.enabled
        || f.workspace_worker_discovery.enabled
        || f.objective.enabled
        || f.manage_workdir.enabled
        || f.workspace_config.enabled
        || f.ticket.enabled
        || f.merge_request.show
        || f.merge_request.open
        || f.merge_request.review
        || f.merge_request.readiness_check
        || f.merge_request.complete
        || f.orchestration.enabled
        || !manifest.mcp.stdio_servers.is_empty()
    {
        return Err(Error::WorkspacePermissionDenied("Backend Job Profile requires authority not granted to this Job (Workspace mutation, Memory, child/peer, Workdir, Flow or MCP)".into()));
    }
    let consolidation = request.grants.subjektiv_consolidation.is_some();
    if f.subjektiv.enabled() != consolidation
        || f.subjektiv.profile.consolidation_tools != consolidation
        || (consolidation && f.subjektiv.profile.extraction.enabled)
    {
        return Err(Error::WorkspacePermissionDenied("Backend Job Profile subjektiv requirements do not match the explicit consolidation grant".into()));
    }
    Ok(())
}
