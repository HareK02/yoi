//! Host-facing, result-only Jobs executed by the private Internal Worker substrate.
//!
//! The host owns persistence, concurrency, timeout, retries and result acceptance. Normal
//! timeout/shutdown routes through the cancellation sender and awaits `run`, including Worker
//! resource cleanup. An emergency drop/abort does not confirm asynchronous cleanup; the Host
//! must retain its lease and reconcile any unknown outcome before allowing another execution.
//! The result-only Feature owns no background tasks, but an in-flight synchronous Host sink
//! cannot be preempted by dropping its tool future and must itself be bounded.

mod result;
pub use result::{JobResultFeature, JobResultFinalizer, JobResultSink, SUBMIT_JOB_RESULT_TOOL};

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use agen::llm_client::LlmClient;
use manifest::{ProfileDiscovery, ProfileResolveOptions, ProfileResolver, Scope, WorkerManifest};

use crate::feature::FeatureRegistryBuilder;
use crate::internal_worker::{
    InternalWorkerAuthority, InternalWorkerIdentity, InternalWorkerSpec,
    run_internal_worker_with_cancel_sender,
};
use crate::{
    PromptCatalog, PromptCatalogSource, SystemPromptContext, SystemPromptTemplate,
    WorkerFilesystemAuthority, WorkerPrompt, WorkerRunResult, WorkerWorkspaceContext,
};

/// Host adapter ceilings, applied even if the selected Profile leaves budgets unbounded.
pub const MAX_JOB_TURNS: u32 = 32;
pub const MAX_JOB_TOKENS: u32 = 8192;

#[derive(Debug, thiserror::Error)]
pub enum JobExecutionError {
    #[error(transparent)]
    Request(#[from] ::job::JobError),
    #[error("Job Profile resolution failed: {0}")]
    Profile(#[from] manifest::ProfileError),
    #[error("result-only Job cannot satisfy Profile requirements: {0}")]
    UnsupportedProfile(String),
    #[error("Job instruction resolution failed: {0}")]
    Instruction(#[from] crate::SystemPromptError),
    #[error("Job Prompt catalog resolution failed: {0}")]
    Catalog(#[from] crate::CatalogError),
    #[error("Job model configuration failed: {0}")]
    Model(#[from] crate::ProviderError),
    #[error("Job execution failed: {0}")]
    Worker(String),
    /// A sink error can follow durable acceptance. Hosts must reconcile their store and must
    /// not classify this as a known retryable failure unless they can prove non-acceptance.
    #[error(
        "Host Job result acceptance is unconfirmed; reconcile the Host store before retrying: {0}"
    )]
    ResultUnconfirmed(String),
    #[error("Job execution was cancelled without an accepted result")]
    Cancelled,
    #[error("Job ended without an accepted SubmitJobResult; final prose is not success")]
    MissingResult,
}

/// Confirms only a synchronous, successful Host acceptance. Lifecycle/prose cannot create this.
#[derive(Debug)]
pub struct InternalJobResult {
    pub submission: ::job::JobResultSubmission,
    pub usage: Option<agen::timeline::event::UsageEvent>,
}

/// Resolved policy/model/prompt, with no running resources and no inherited authority.
pub struct PreparedInternalJob {
    request: ::job::JobRequest,
    manifest: WorkerManifest,
    client: Box<dyn LlmClient>,
    system_prompt: String,
    features: FeatureRegistryBuilder,
    finalizer: Option<Arc<dyn JobResultFinalizer>>,
}

/// Explicit Host-supplied domain modules and the Profile requirements they satisfy.
/// Profile names never issue this grant. The Host must bind all module operations to
/// the immutable request/attempt and fence them on cancellation, timeout and shutdown.
#[derive(Default)]
pub struct JobFeatureGrant {
    pub features: FeatureRegistryBuilder,
    pub satisfied_requirements: Vec<&'static str>,
    pub finalizer: Option<Arc<dyn JobResultFinalizer>>,
}

pub fn prepare_job_with_grant(
    request: &::job::JobRequest,
    cwd: &Path,
    client: Option<Box<dyn LlmClient>>,
    grant: JobFeatureGrant,
) -> Result<PreparedInternalJob, JobExecutionError> {
    let registry = ProfileDiscovery::user_settings().discover()?;
    prepare_job_from_registry_with_grant(request, cwd, client, &registry, grant)
}

/// Resolve exactly the requested registry Profile using the existing user settings registry.
/// `cwd` is a resolution base only; it never becomes filesystem or Workspace authority.
/// An injected client is useful for scripted executions and does not bypass policy validation.
pub fn prepare_job(
    request: &::job::JobRequest,
    cwd: &Path,
    client: Option<Box<dyn LlmClient>>,
) -> Result<PreparedInternalJob, JobExecutionError> {
    let registry = ProfileDiscovery::user_settings().discover()?;
    prepare_job_from_registry(request, cwd, client, &registry)
}

fn prepare_job_from_registry(
    request: &::job::JobRequest,
    cwd: &Path,
    client: Option<Box<dyn LlmClient>>,
    registry: &manifest::ProfileRegistry,
) -> Result<PreparedInternalJob, JobExecutionError> {
    prepare_job_from_registry_with_grant(request, cwd, client, registry, JobFeatureGrant::default())
}

fn prepare_job_from_registry_with_grant(
    request: &::job::JobRequest,
    cwd: &Path,
    client: Option<Box<dyn LlmClient>>,
    registry: &manifest::ProfileRegistry,
    grant: JobFeatureGrant,
) -> Result<PreparedInternalJob, JobExecutionError> {
    request.validate()?;
    let resolved = ProfileResolver::new()
        .with_workspace_base(cwd)
        .resolve_from_registry(
            &request.profile_selector()?,
            registry,
            ProfileResolveOptions::with_worker_name("internal-job"),
        )?;
    let mut manifest = resolved.manifest;
    validate_profile_with_grant(&manifest, &grant.satisfied_requirements)?;
    if let Some(finalizer) = &grant.finalizer {
        finalizer
            .validate_profile(&manifest)
            .map_err(JobExecutionError::UnsupportedProfile)?;
    }
    let mut tool_names = vec![SUBMIT_JOB_RESULT_TOOL.to_string()];
    for descriptor in grant.features.descriptors() {
        tool_names.extend(descriptor.tools.iter().map(|tool| tool.name.clone()));
    }
    let scope = Scope::empty();
    let prompts = PromptCatalog::builtins_only()?;
    let template = SystemPromptTemplate::parse(
        &manifest.engine.instruction,
        PromptCatalogSource::builtins_only(),
    )?;
    let mut system_prompt = template.render(&SystemPromptContext {
        now: chrono::Utc::now(),
        cwd: Cow::Borrowed("(no filesystem)"),
        language: &manifest.engine.language,
        scope: &scope,
        tool_names,
        feature_instructions: &[],
        agents_md: None,
        resident_summary: None,
        prompts: &prompts,
    })?;
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&prompts.render(WorkerPrompt::JobSystem, minijinja::context! {})?);
    // Host budgets attenuate Profile output/turn policy, never grant another capability.
    manifest.engine.max_turns = std::num::NonZeroU32::new(
        manifest
            .engine
            .max_turns
            .map(|n| n.get())
            .unwrap_or(MAX_JOB_TURNS)
            .min(MAX_JOB_TURNS),
    );
    manifest.engine.max_tokens = Some(
        manifest
            .engine
            .max_tokens
            .unwrap_or(MAX_JOB_TOKENS)
            .min(MAX_JOB_TOKENS),
    );
    let model_config = manifest::model_catalog::resolve_model_manifest(&manifest.model)
        .map_err(crate::ProviderError::from)?;
    let client = match client {
        Some(client) => client,
        None => crate::model_client::build_client_from_config(&model_config)?,
    };
    Ok(PreparedInternalJob {
        request: request.clone(),
        manifest,
        client,
        system_prompt,
        features: grant.features,
        finalizer: grant.finalizer,
    })
}

impl PreparedInternalJob {
    /// Run one immutable attempt. The sink must perform fenced, durable acceptance before
    /// returning Ok. Timeout is Host-owned: cancel and then await this future for cleanup.
    /// A result already accepted by the sink remains success even if the model subsequently
    /// fails, reaches its turn limit or is cancelled; callers never rerun an accepted result.
    pub async fn run<F>(
        self,
        attempt_id: String,
        submit: Arc<dyn JobResultSink>,
        on_cancel_sender: F,
    ) -> Result<InternalJobResult, JobExecutionError>
    where
        F: FnOnce(tokio::sync::mpsc::Sender<()>),
    {
        let mut feature = JobResultFeature::new(&self.request, attempt_id.clone(), submit)?;
        if let Some(finalizer) = self.finalizer {
            feature =
                feature.with_finalizer(finalizer, self.manifest.clone(), self.client.clone_boxed());
        }
        let accepted = feature.accepted();
        let engine_policy = self.manifest.engine.clone();
        let spec = InternalWorkerSpec {
            identity: InternalWorkerIdentity {
                kind: "job",
                run_id: uuid::Uuid::now_v7(),
            },
            max_turns: engine_policy.max_turns.map(|n| n.get()),
            engine_configurator: Some(Box::new(move |engine| {
                crate::apply_worker_manifest(engine, &engine_policy);
            })),
            manifest: self.manifest,
            client: self.client,
            system_prompt: self.system_prompt,
            input: self.request.worker_input(&attempt_id)?,
            cache_key: None,
            features: self.features.with_module(feature.clone()),
            required_tools: &[SUBMIT_JOB_RESULT_TOOL],
            authority: InternalWorkerAuthority {
                workspace: WorkerWorkspaceContext::no_workspace(),
                filesystem: WorkerFilesystemAuthority::None,
                scope: Scope::empty(),
                workdir_session: None,
            },
        };
        let outcome = run_internal_worker_with_cancel_sender(spec, on_cancel_sender).await;
        let usage = match &outcome {
            Ok(run) => run.usage.clone(),
            Err(error) => error.usage.clone(),
        };
        if let Some(submission) = accepted.lock().expect("Job result state poisoned").clone() {
            return Ok(InternalJobResult { submission, usage });
        }
        if let Some(error) = feature.unconfirmed() {
            return Err(JobExecutionError::ResultUnconfirmed(error));
        }
        match outcome {
            Err(error) => Err(JobExecutionError::Worker(error.source.to_string())),
            Ok(run)
                if matches!(
                    run.lifecycle,
                    WorkerRunResult::Cancelled | WorkerRunResult::RolledBack
                ) =>
            {
                Err(JobExecutionError::Cancelled)
            }
            Ok(_) => Err(JobExecutionError::MissingResult),
        }
    }
}

/// Validate requirements, not Profile names. Nothing configured as active is silently stripped.

fn validate_profile_with_grant(
    manifest: &WorkerManifest,
    satisfied: &[&str],
) -> Result<(), JobExecutionError> {
    let manifest::FeatureConfig {
        task,
        memory,
        subjektiv,
        web,
        image,
        sub_worker,
        flow,
        worker,
        workspace_worker_discovery,
        objective,
        manage_workdir,
        workdir_catalog,
        workspace_config,
        ticket,
        merge_request,
        orchestration,
    } = &manifest.feature;
    let mut unavailable = Vec::new();
    for (name, enabled) in [
        ("feature.task", task.enabled),
        (
            "feature.memory",
            memory.profile.enabled
                || memory.profile.staging_tools
                || memory.workspace_settings.is_some(),
        ),
        (
            "feature.subjektiv",
            subjektiv.profile.enabled
                || subjektiv.profile.consolidation_tools
                || subjektiv.workspace_settings.is_some(),
        ),
        ("feature.web", web.enabled),
        ("feature.image", image.enabled),
        ("feature.sub_worker", sub_worker.enabled),
        ("feature.flow", flow.enabled),
        ("feature.worker", worker.enabled),
        (
            "feature.workspace_worker_discovery",
            workspace_worker_discovery.enabled,
        ),
        ("feature.objective", objective.enabled),
        ("feature.manage_workdir", manage_workdir.enabled),
        ("feature.workspace_config", workspace_config.enabled),
        (
            "feature.ticket",
            ticket.enabled || ticket.authoring || ticket.thread || ticket.intake || ticket.workflow,
        ),
        (
            "feature.merge_request",
            merge_request.show
                || merge_request.open
                || merge_request.review
                || merge_request.readiness_check
                || merge_request.complete,
        ),
        ("feature.orchestration", orchestration.enabled),
        // The default WIP-only catalog is inert in Tools mode, not a tool requirement.
        (
            "feature.workdir_catalog",
            workdir_catalog.enabled && manifest.worker.mode == manifest::WorkerMode::Wip,
        ),
        (
            "worker.mode=wip",
            manifest.worker.mode == manifest::WorkerMode::Wip,
        ),
        ("scope", !manifest.scope.allow.is_empty()),
        (
            "delegation_scope",
            !manifest.delegation_scope.allow.is_empty(),
        ),
        ("mcp", !manifest.mcp.stdio_servers.is_empty()),
        ("skills", manifest.skills.is_some()),
        ("compaction", manifest.compaction.is_some()),
        (
            "web",
            manifest
                .web
                .as_ref()
                .is_some_and(|config| config.enabled == Some(true)),
        ),
        (
            "session.record_event_trace",
            manifest.session.record_event_trace,
        ),
    ] {
        if enabled && !satisfied.contains(&name) {
            unavailable.push(name);
        }
    }
    if unavailable.is_empty() {
        Ok(())
    } else {
        Err(JobExecutionError::UnsupportedProfile(
            unavailable.join(", "),
        ))
    }
}

#[cfg(test)]
mod tests;
