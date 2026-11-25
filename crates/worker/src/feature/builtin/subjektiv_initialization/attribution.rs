//! Owner-side Session attribution and retention. New/repair operations persist
//! CAS-fenced receipts; ordinary restore preparation only checks saved state.

use super::{InitializationPhase, ResolvedWorkerConnection, WorkerConnectionKind};
use crate::{Worker, WorkerError};
use agen::llm_client::client::LlmClient;
use session_store::{SessionId, Store, SubjektivSessionAttributionState, WorkerMetadataStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjektivSessionAttributionLifecycle {
    NewSession,
    RestoredSession,
}

fn incomplete_subjektiv_attribution_state(
    session_id: SessionId,
    create_if_missing: bool,
    diagnostic: String,
) -> SubjektivSessionAttributionState {
    if create_if_missing {
        SubjektivSessionAttributionState::OutcomeUnknown {
            session_id,
            diagnostic,
        }
    } else {
        SubjektivSessionAttributionState::LegacyUnknown {
            session_id,
            diagnostic,
        }
    }
}

fn bounded_attribution_diagnostic(mut diagnostic: String) -> String {
    const MAX_CHARS: usize = 1024;
    if diagnostic.chars().count() > MAX_CHARS {
        diagnostic = diagnostic.chars().take(MAX_CHARS).collect();
        diagnostic.push('…');
    }
    diagnostic
}

impl<C: LlmClient + 'static, St: Store + WorkerMetadataStore> Worker<C, St> {
    /// Check saved Session attribution without contacting the Host. Restoring a
    /// Controller reconstructs existing state, not current Subject authority.
    /// Protected operations check current authority at their own boundaries.
    pub(crate) fn validate_saved_subjektiv_attribution(&self) -> Result<(), WorkerError> {
        if self
            .backend_job()
            .is_some_and(crate::BackendJobExecutionBinding::has_subjektiv_job_grant)
        {
            return Ok(());
        }
        let Some(metadata) = self.store().read_by_name(&self.manifest().worker.name)? else {
            return Ok(());
        };
        let Some(attribution) = metadata.subjektiv_session_attribution.as_ref() else {
            return Ok(());
        };
        let denied = |message: String| WorkerError::SubjektivSessionAttribution {
            state: "publication_denied",
            message: bounded_attribution_diagnostic(message),
        };
        let session_id = self.session_id();
        if metadata.active.as_ref().map(|active| active.session_id) != Some(session_id)
            || attribution.session_id() != session_id
        {
            return Err(denied(
                "retained body attribution does not match the constructed Session".into(),
            ));
        }
        let SubjektivSessionAttributionState::Confirmed { subject_id, .. } = attribution else {
            return Err(denied(
                "body publication requires separately confirmed Session attribution".into(),
            ));
        };
        if subject_id.trim().is_empty() || subject_id.chars().any(char::is_control) {
            return Err(denied(
                "saved Session attribution has an invalid Subject identity".into(),
            ));
        }
        Ok(())
    }
}

impl<C, St> Worker<C, St>
where
    C: LlmClient + 'static,
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    /// Complete the Host-owned Session-to-Subject attribution boundary before a
    /// runtime-managed Worker is exposed or accepts input. The authoritative
    /// attachment decision and lifecycle kind come from the owning adapter; names,
    /// Profiles and current singleton ownership are never historical identity.
    ///
    /// New Sessions persist `OutcomeUnknown` before create-if-missing network I/O.
    /// A restore retries creation only for that durable state (or `Failed`). An
    /// unmarked legacy restore is verification-only and persists `LegacyUnknown`,
    /// so current ownership can never invent historical attribution.
    pub fn finalize_subjektiv_session_attribution(
        &self,
        subjektiv_attached: bool,
        lifecycle: SubjektivSessionAttributionLifecycle,
    ) -> Result<(), WorkerError> {
        self.finalize_subjektiv_session_attribution_observed(
            subjektiv_attached,
            lifecycle,
            None,
            &mut None,
        )
    }

    /// Retain a failed startup against its constructed Session and Workspace. Only
    /// this operation's successful CAS transitions advance the attribution expectation;
    /// a reread never adopts a replacement owner or another operation's attribution.
    pub fn finalize_subjektiv_session_attribution_retaining_startup_failure(
        &self,
        subjektiv_attached: bool,
        lifecycle: SubjektivSessionAttributionLifecycle,
    ) -> Result<(), WorkerError> {
        let mut expected = None;
        self.finalize_subjektiv_session_attribution_observed(
            subjektiv_attached,
            lifecycle,
            None,
            &mut expected,
        )
        .map_err(|error| match expected {
            Some(expected) => crate::startup_failure::persist_binding_startup_failure(
                self.store(),
                &expected,
                error,
            ),
            None => error,
        })
    }

    pub(crate) fn finalize_subjektiv_connection(
        &self,
        connection: Option<&ResolvedWorkerConnection>,
        phase: InitializationPhase,
    ) -> Result<(), WorkerError> {
        let lifecycle = match phase {
            InitializationPhase::New => SubjektivSessionAttributionLifecycle::NewSession,
            InitializationPhase::Restored | InitializationPhase::Recovery => {
                SubjektivSessionAttributionLifecycle::RestoredSession
            }
        };
        let expected_subject = connection
            .filter(|c| c.kind == WorkerConnectionKind::SubjectBody)
            .and_then(|c| c.subject_id.as_deref());
        let mut observed = None;
        let result = self.finalize_subjektiv_session_attribution_observed(
            connection.is_some(),
            lifecycle,
            expected_subject,
            &mut observed,
        );
        result.map_err(|source| match (phase, observed) {
            (InitializationPhase::New | InitializationPhase::Recovery, Some(expected)) => {
                crate::startup_failure::persist_binding_startup_failure(
                    self.store(),
                    &expected,
                    source,
                )
            }
            _ => source,
        })
    }

    fn finalize_subjektiv_session_attribution_observed(
        &self,
        subjektiv_attached: bool,
        lifecycle: SubjektivSessionAttributionLifecycle,
        expected_subject: Option<&str>,
        observation: &mut Option<session_store::worker_metadata::StartupFailureExpectation>,
    ) -> Result<(), WorkerError> {
        let _admission = self.mutation_authority().enter()?;
        let worker_name = self.manifest().worker.name.as_str();
        let metadata = self.store().read_by_name(worker_name)?.ok_or_else(|| {
            WorkerError::SubjektivSessionAttribution {
                state: "metadata_missing",
                message: "Worker metadata disappeared before Session attribution".to_string(),
            }
        })?;
        let active =
            metadata
                .active
                .as_ref()
                .ok_or_else(|| WorkerError::SubjektivSessionAttribution {
                    state: "session_missing",
                    message: "Worker metadata has no active Session attribution target".to_string(),
                })?;
        let session_id = self.session_id();
        if active.session_id != session_id {
            return Err(WorkerError::SubjektivSessionAttribution {
                state: "session_mismatch",
                message: format!(
                    "Worker metadata Session {} does not match constructed Session {session_id}",
                    active.session_id
                ),
            });
        }

        if metadata.worker_name != worker_name
            || metadata.workspace_id.as_deref() != self.workspace_id().map(|id| id.as_str())
        {
            return Err(WorkerError::SubjektivSessionAttribution {
                state: "owner_mismatch",
                message: "Worker metadata does not match constructed Workspace ownership".into(),
            });
        }
        *observation =
            session_store::worker_metadata::StartupFailureExpectation::from_metadata(&metadata);
        let mut persist_attribution =
            |previous: Option<&SubjektivSessionAttributionState>,
             next: SubjektivSessionAttributionState| {
                let expected = observation
                    .as_mut()
                    .expect("validated active Session has an expectation");
                if expected.attribution.as_ref() != previous {
                    return Err(WorkerError::SubjektivSessionAttribution {
                        state: "attribution_conflict",
                        message: "startup attribution expectation changed".into(),
                    });
                }
                crate::subjektiv_metadata::compare_and_swap_attribution(
                    self.store(),
                    expected,
                    next,
                )
            };

        if self
            .backend_job()
            .is_some_and(crate::BackendJobExecutionBinding::has_subjektiv_job_grant)
        {
            if metadata.subjektiv_session_attribution.is_some() {
                return Err(WorkerError::SubjektivSessionAttribution {
                    state: "job_attribution_forbidden",
                    message: "Subject maintenance Job execution must not carry subject-body Session attribution".into(),
                });
            }
            return Ok(());
        }
        if !subjektiv_attached {
            if metadata.subjektiv_session_attribution.is_some() {
                return Err(WorkerError::SubjektivSessionAttribution {
                    state: "attachment_missing",
                    message:
                        "persisted Subjektiv Session attribution state requires an authorized attachment"
                            .to_string(),
                });
            }
            return Ok(());
        }

        let create_if_missing = match metadata.subjektiv_session_attribution.as_ref() {
            Some(state) => {
                if state.session_id() != session_id {
                    return Err(WorkerError::SubjektivSessionAttribution {
                        state: "session_mismatch",
                        message: format!(
                            "persisted Subjektiv attribution targets Session {}, not {session_id}",
                            state.session_id()
                        ),
                    });
                }
                match state {
                    SubjektivSessionAttributionState::Confirmed { subject_id, .. } => {
                        if expected_subject.is_some_and(|expected| expected != subject_id) {
                            return Err(WorkerError::SessionRegistration { source: crate::subjektiv::SubjektivHostError::SessionRegistration {
                                cause: protocol::session_failure::SessionRegistrationCause::binding_mismatch(), source: None,
                            }});
                        }
                        return self
                            .retain_journal_source_before_publication(session_id, subject_id);
                    }
                    SubjektivSessionAttributionState::OutcomeUnknown { .. }
                    | SubjektivSessionAttributionState::Failed { .. } => true,
                    SubjektivSessionAttributionState::LegacyUnknown { .. } => false,
                }
            }
            None => matches!(lifecycle, SubjektivSessionAttributionLifecycle::NewSession),
        };
        let initial_diagnostic = if create_if_missing {
            "attribution creation started; completion is not yet confirmed"
        } else {
            "legacy Session has no local attribution evidence; immutable Backend verification started"
        };
        let pending_attribution = incomplete_subjektiv_attribution_state(
            session_id,
            create_if_missing,
            initial_diagnostic.to_string(),
        );
        persist_attribution(
            metadata.subjektiv_session_attribution.as_ref(),
            pending_attribution.clone(),
        )?;
        let output = if self.explicit_subjektiv_host().is_some() {
            self.subjektiv_host_connection()
                .map_err(|error| crate::subjektiv::SubjektivHostError::session_registration(
                    protocol::session_failure::SessionRegistrationCause::outcome_unknown(), error,
                ))
                .and_then(|connection| connection.ok_or_else(|| {
                    crate::subjektiv::SubjektivHostError::SessionRegistration {
                        cause: protocol::session_failure::SessionRegistrationCause::binding_mismatch(),
                        source: None,
                    }
                }))
                .and_then(|connection| connection.host.record_session(&connection.context, create_if_missing))
        } else {
            crate::subjektiv::record_backend_session(
                self.workspace_client(), &session_id.to_string(), create_if_missing,
            )
        }.and_then(|output| crate::subjektiv::validate_session_registration(output, &session_id.to_string()))
        .and_then(|output| {
            if expected_subject.is_some_and(|expected| expected != output.subject_id) {
                return Err(crate::subjektiv::SubjektivHostError::SessionRegistration {
                    cause: protocol::session_failure::SessionRegistrationCause::binding_mismatch(), source: None,
                });
            }
            Ok(output)
        });
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                use protocol::session_failure::SessionRegistrationReason;
                let cause = error.session_registration_cause();
                let rejected = matches!(
                    cause.reason,
                    SessionRegistrationReason::PermissionDenied
                        | SessionRegistrationReason::AttributionMissing
                        | SessionRegistrationReason::BindingMismatch
                );
                // The legacy lifecycle diagnostic is a fixed domain label, never a
                // transport body, credential, path or upstream Display string.
                let diagnostic = cause.to_string();
                let next = if create_if_missing && rejected {
                    SubjektivSessionAttributionState::Failed {
                        session_id,
                        diagnostic,
                    }
                } else {
                    incomplete_subjektiv_attribution_state(
                        session_id,
                        create_if_missing,
                        diagnostic,
                    )
                };
                let source = WorkerError::SessionRegistration { source: error };
                let saved = persist_attribution(Some(&pending_attribution), next);
                return Err(crate::startup_failure::retain_attribution_save_failure(
                    source, saved,
                ));
            }
        };
        persist_attribution(
            Some(&pending_attribution),
            SubjektivSessionAttributionState::Confirmed {
                session_id,
                subject_id: output.subject_id.clone(),
            },
        )?;
        self.retain_journal_source_before_publication(session_id, &output.subject_id)
    }

    fn retain_journal_source_before_publication(
        &self,
        session_id: SessionId,
        subject_id: &str,
    ) -> Result<(), WorkerError> {
        let Some(connection) = self.subjektiv_host_connection().map_err(|error| {
            WorkerError::SubjektivSessionAttribution {
                state: "retention_unconfirmed",
                message: error.to_string(),
            }
        })?
        else {
            return Ok(());
        };
        if !connection.host.journal_recording_granted() {
            return Ok(());
        }
        let pin = session_store::retention::SessionRetentionPin(
            match self.workspace_client().workspace_id() {
                Some(workspace) => subjektiv::journal::source_retention_pin(
                    workspace,
                    subject_id,
                    &session_id.to_string(),
                ),
                None => session_id,
            },
        );
        // Confirmation can be replayed after response loss. Persist this intent before
        // exposing the Worker to input, not after the first RunCommitted notification.
        // It does not claim that an original Session has materialized yet.
        self.store()
            .retain_session_before_publication(session_id, pin)?;
        Ok(())
    }
}
