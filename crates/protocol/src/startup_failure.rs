//! Startup diagnostics are observations, never lifecycle, ownership, or retry authority.
//!
//! Attachments contain a domain's explicitly projected error-contract envelope. Receivers
//! decode through the existing codec, and must project again for a different audience;
//! deserializing stored bytes never creates a disclosure capability. Unknown and corrupt
//! attachments remain readable alongside metadata instead of breaking its lifecycle fields.
use error_contract::{
    Audience, Budget, ContractError, DecodeFailure, ProjectedFailure, WireDetails, decode_failure,
    encode_projected_failure,
};
use serde::{Deserialize, Serialize};

/// A bounded, projected envelope at the Worker/Runtime or metadata boundary.
/// No constructor accepts Display text, arbitrary JSON fields, or an unprojected error.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct StartupFailureAttachment(String);

impl std::fmt::Debug for StartupFailureAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartupFailureAttachment")
            .finish_non_exhaustive()
    }
}

impl StartupFailureAttachment {
    pub fn from_projected<A: Audience, D: WireDetails>(
        failure: &ProjectedFailure<A, D>,
    ) -> Result<Self, ContractError> {
        let encoded = encode_projected_failure(failure, Self::budget())?;
        String::from_utf8(encoded.as_bytes().to_vec())
            .map(Self)
            .map_err(|_| ContractError::Encoding)
    }

    /// Missing attachments are represented by the enclosing Option, not a fabricated cause.
    pub fn decode<D: WireDetails>(&self) -> DecodeFailure<D> {
        decode_failure(self.0.as_bytes(), Self::budget())
    }

    /// Redisclosure requires a fresh domain/audience projection and current authorization.
    /// Unsupported or corrupt stored bytes are never forwarded as public details.
    pub fn project_for<A: Audience, D: WireDetails + error_contract::ProjectFor<A>>(
        &self,
        authorization: &D::Authorization,
    ) -> Result<ProjectedFailure<A, D::Details>, ContractError> {
        match self.decode::<D>() {
            DecodeFailure::Known(received) => {
                error_contract::project_failure(received.facts(), authorization)
            }
            DecodeFailure::Unsupported(_) | DecodeFailure::Corrupt(_) => {
                Err(ContractError::ProjectionUnavailable)
            }
        }
    }

    fn budget() -> Budget {
        Budget::new(
            error_contract::MAX_FAILURE_BYTES,
            error_contract::MAX_FAILURE_BYTES,
        )
    }
}

/// Storage encoding cannot be constructed by relabeling an internal-transfer attachment.
/// A loaded value remains untrusted evidence; publication still requires `project_for`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct StoredStartupFailureAttachment(StartupFailureAttachment);

impl StoredStartupFailureAttachment {
    pub fn from_projected<D: WireDetails>(
        failure: &ProjectedFailure<error_contract::Storage, D>,
    ) -> Result<Self, ContractError> {
        StartupFailureAttachment::from_projected(failure).map(Self)
    }
    pub fn decode<D: WireDetails>(&self) -> DecodeFailure<D> {
        self.0.decode()
    }
    pub fn project_for<A: Audience, D: WireDetails + error_contract::ProjectFor<A>>(
        &self,
        authorization: &D::Authorization,
    ) -> Result<ProjectedFailure<A, D::Details>, ContractError> {
        self.0.project_for::<A, D>(authorization)
    }
}

/// Owner-selected startup stage; independent of resource cleanup disposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupStage {
    Preflight,
    Metadata,
    Reconstruction,
    Recovery,
    Binding,
    Feature,
    ActivationEffects,
    Controller,
}

/// Safe local Runtime cause when no more specific domain attachment was supplied.
/// The primary cause is selected where the failure occurs, not parsed from its diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RuntimeStartupCause {
    pub stage: StartupStage,
    pub reason: RuntimeStartupReason,
}

/// Coarse local causes do not claim a domain authorization verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum RuntimeStartupReason {
    StartupFailed,
    SegmentEmpty,
    SystemPromptMissing,
    MetadataPending,
    MetadataMissing,
    MetadataInactive,
    ManifestMissing,
    AllocationFailed,
    SessionReadFailed,
    MetadataReadFailed,
    ProviderFailed,
    AttributionFailed,
    FeatureFailed,
}

impl error_contract::FailureCause for RuntimeStartupCause {
    fn identity(&self) -> error_contract::FailureIdentity {
        error_contract::FailureIdentity {
            domain: "worker_startup",
            reason: "failed",
            category: error_contract::Category::Internal,
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WireDetails for RuntimeStartupCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "worker_startup" && reason == "failed" && version == 1
    }
}

// This cause contains only an enum. Arbitrary provider messages, IO paths and credentials
// cannot enter its projection. Domain-specific causes must supply their own projector.
macro_rules! project_runtime {
    ($($audience:ty),*) => {$(
        impl error_contract::ProjectFor<$audience> for RuntimeStartupCause {
            type Authorization = ();
            type Details = Self;
            fn disclose(&self, facts: &error_contract::FailureFacts<Self>, _: &())
                -> Result<error_contract::Disclosure<Self>, ContractError>
            {
                let mut disclosure = error_contract::Disclosure::new(self.clone(), "Worker startup failed.");
                // Only fixed owner-selected operation/stage labels are published here.
                for context in &facts.context().operations {
                    if context.operation == "worker_startup" && context.stage == "runtime" {
                        disclosure.context.operations.push(context.clone());
                        if let Some(last) = disclosure.context.operations.last_mut() {
                            last.operation_ref = None;
                        }
                    }
                }
                Ok(disclosure)
            }
        }
    )*};
}
project_runtime!(
    error_contract::InternalTransfer,
    error_contract::Storage,
    error_contract::Browser
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupFailurePhase {
    Cleanup,
    RuntimeRecordDelete,
    RuntimeSnapshotSave,
    RuntimeRemovalPublish,
    MetadataSave,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupCleanupFailure {
    Ownership,
    Workdir,
    Artifacts,
    Allocation,
    Feature,
    Unsupported,
}

/// A separate secondary fact, not a replacement primary cause or a causal-chain link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupSecondaryFailure {
    ProjectionUnavailable {
        phase: StartupFailurePhase,
    },
    Cleanup {
        failure: StartupCleanupFailure,
    },
    Persistence {
        phase: StartupFailurePhase,
        outcome: StartupPersistenceOutcome,
    },
}

/// Persisted is evidence only for this attachment/record write, not execution completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupPersistenceOutcome {
    Persisted,
    NotSaved,
    Conflict,
    OutcomeUnknown,
}

/// Independent resource observations from the existing execution owner. These diagnostics
/// never promote a result to RolledBack or authorize replay; the backend disposition does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupResources {
    None,
    WorkerStartup,
    Unproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupCleanup {
    NotNeeded,
    Required,
    Confirmed,
    Failed(StartupCleanupFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct StartupResourceFacts {
    pub resources: StartupResources,
    pub cleanup: StartupCleanup,
    pub durable_effects_possible: bool,
}

/// Path-free IO classification. Nonportable OS details remain at the resource owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum StartupIoKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidInput,
    InvalidData,
    Interrupted,
    TimedOut,
    WouldBlock,
    UnexpectedEof,
    Other,
}
impl From<std::io::ErrorKind> for StartupIoKind {
    fn from(kind: std::io::ErrorKind) -> Self {
        use std::io::ErrorKind;
        match kind {
            ErrorKind::NotFound => Self::NotFound,
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::AlreadyExists => Self::AlreadyExists,
            ErrorKind::InvalidInput => Self::InvalidInput,
            ErrorKind::InvalidData => Self::InvalidData,
            ErrorKind::Interrupted => Self::Interrupted,
            ErrorKind::TimedOut => Self::TimedOut,
            ErrorKind::WouldBlock => Self::WouldBlock,
            ErrorKind::UnexpectedEof => Self::UnexpectedEof,
            _ => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use error_contract::{
        Browser, FailureFacts, InternalTransfer, OperationContext, Storage, project_failure,
    };

    #[test]
    fn startup_projection_preserves_allowlisted_facts_without_secret_context() {
        let mut facts = FailureFacts::new(RuntimeStartupCause {
            stage: StartupStage::Binding,
            reason: RuntimeStartupReason::AttributionFailed,
        })
        .unwrap();
        let mut context = OperationContext::new("worker_startup", "runtime").unwrap();
        context.operation_ref = Some("TEST_ONLY_CREDENTIAL".into());
        facts.with_context(context).unwrap();
        facts
            .with_context(OperationContext::new("/private/internal/path", "raw_http_body").unwrap())
            .unwrap();
        let transfer = project_failure::<InternalTransfer, _>(&facts, &()).unwrap();
        let attachment = StartupFailureAttachment::from_projected(&transfer).unwrap();
        let storage_projection = attachment
            .project_for::<Storage, RuntimeStartupCause>(&())
            .unwrap();
        let stored = StoredStartupFailureAttachment::from_projected(&storage_projection).unwrap();
        let browser = stored
            .project_for::<Browser, RuntimeStartupCause>(&())
            .unwrap();
        assert_eq!(browser.details(), facts.cause());
        assert_eq!(browser.context().operations.len(), 1);
        assert_eq!(browser.context().operations[0].operation_ref, None);
        let public =
            encode_projected_failure(&browser, StartupFailureAttachment::budget()).unwrap();
        let wire = std::str::from_utf8(public.as_bytes()).unwrap();
        for secret in [
            "TEST_ONLY_CREDENTIAL",
            "/private/internal/path",
            "raw_http_body",
        ] {
            assert!(!wire.contains(secret));
            assert!(!serde_json::to_string(&stored).unwrap().contains(secret));
        }
        assert!(matches!(
            stored.decode::<RuntimeStartupCause>(),
            DecodeFailure::Known(_)
        ));
    }

    #[test]
    fn unsupported_and_corrupt_startup_attachments_cannot_be_redisclosed() {
        let facts = FailureFacts::new(RuntimeStartupCause {
            stage: StartupStage::Controller,
            reason: RuntimeStartupReason::StartupFailed,
        })
        .unwrap();
        let projected = project_failure::<InternalTransfer, _>(&facts, &()).unwrap();
        let attachment = StartupFailureAttachment::from_projected(&projected).unwrap();
        let future =
            StartupFailureAttachment(attachment.0.replace("\"version\":1", "\"version\":999"));
        assert!(matches!(
            future.decode::<RuntimeStartupCause>(),
            DecodeFailure::Unsupported(_)
        ));
        assert!(
            future
                .project_for::<Browser, RuntimeStartupCause>(&())
                .is_err()
        );
        let corrupt = StartupFailureAttachment("{invalid TEST_ONLY_SECRET".into());
        assert!(matches!(
            corrupt.decode::<RuntimeStartupCause>(),
            DecodeFailure::Corrupt(_)
        ));
        assert!(
            corrupt
                .project_for::<Browser, RuntimeStartupCause>(&())
                .is_err()
        );
        assert!(!format!("{corrupt:?}").contains("TEST_ONLY_SECRET"));
    }
}
