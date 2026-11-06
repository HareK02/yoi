//! Attempt-local failure evidence, not execution, retry, or cleanup authority.
//!
//! v9 adds nullable attachments without interpreting historical `failure` prose.
//! Loaded attachments never implement Serialize; queries must reproject with the
//! caller's CURRENT visibility. The fixed vocabulary deliberately stops at the
//! Job runner boundary; private Worker errors/cleanup handles remain owner-held.
use error_contract::{
    Audience, Browser, Category, ContractError, DecodeFailure, Disclosure, FailureCause,
    FailureFacts, FailureIdentity, InternalTransfer, ProjectFor, ProjectedFailure, Storage, Tool,
    WireDetails, project_failure,
};
use protocol::background_failure::{BackgroundFailure, StoredFailure};
use serde::{Deserialize, Serialize};

/// Closed mapping of the actual JobExecutionError variants and Host observations.
/// No paths, prompts, provider bodies, or Display strings are representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobFailureCause {
    Request,
    Profile,
    UnsupportedProfile,
    Instruction,
    Catalog,
    Model,
    Worker,
    ResultUnconfirmed,
    Cancelled,
    MissingResult,
    DeadlineExceeded,
    HostStopped,
    CleanupUnconfirmed,
}

impl JobFailureCause {
    pub(crate) fn execution(error: &worker::job::JobExecutionError) -> Self {
        use worker::job::JobExecutionError as E;
        match error {
            E::Request(_) => Self::Request,
            E::Profile(_) => Self::Profile,
            E::UnsupportedProfile(_) => Self::UnsupportedProfile,
            E::Instruction(_) => Self::Instruction,
            E::Catalog(_) => Self::Catalog,
            E::Model(_) => Self::Model,
            E::Worker(_) => Self::Worker,
            E::ResultUnconfirmed(_) => Self::ResultUnconfirmed,
            E::Cancelled => Self::Cancelled,
            E::MissingResult => Self::MissingResult,
        }
    }

    pub(crate) fn explanation(self) -> &'static str {
        match self {
            Self::Request => "Job request validation failed.",
            Self::Profile => "Job Profile resolution failed.",
            Self::UnsupportedProfile => "Job Profile requirements cannot be satisfied.",
            Self::Instruction => "Job instruction resolution failed.",
            Self::Catalog => "Job Prompt catalog resolution failed.",
            Self::Model => "Job model configuration failed.",
            Self::Worker => "Job Worker execution failed.",
            Self::ResultUnconfirmed => {
                "Job result acknowledgement was unconfirmed; reconcile the same attempt's saved result."
            }
            Self::Cancelled => "Job execution was cancelled without an accepted result.",
            Self::MissingResult => "Job Worker finished without an accepted structured result.",
            Self::DeadlineExceeded => "Job execution deadline elapsed.",
            Self::HostStopped => "Job Host requested cancellation or shutdown.",
            Self::CleanupUnconfirmed => {
                "Job execution cleanup is unconfirmed; ownership remains retained."
            }
        }
    }

    pub(crate) fn attachment(self) -> Result<BackgroundFailure, ContractError> {
        let facts = FailureFacts::new(self)?;
        BackgroundFailure::from_projected(&project_failure::<Storage, _>(&facts, &())?)
    }
}

impl FailureCause for JobFailureCause {
    fn identity(&self) -> FailureIdentity {
        let (reason, category) = match self {
            Self::Request => ("request", Category::InvalidInput),
            Self::Profile => ("profile", Category::InvalidInput),
            Self::UnsupportedProfile => ("unsupported_profile", Category::InvalidInput),
            Self::Instruction => ("instruction", Category::InvalidInput),
            Self::Catalog => ("catalog", Category::Dependency),
            Self::Model => ("model", Category::Dependency),
            Self::Worker => ("worker", Category::Internal),
            Self::ResultUnconfirmed => ("result_unconfirmed", Category::Unknown),
            Self::Cancelled => ("cancelled", Category::Internal),
            Self::MissingResult => ("missing_result", Category::Internal),
            Self::DeadlineExceeded => ("deadline_exceeded", Category::Internal),
            Self::HostStopped => ("host_stopped", Category::Internal),
            Self::CleanupUnconfirmed => ("cleanup_unconfirmed", Category::Unknown),
        };
        FailureIdentity {
            domain: "standalone_job",
            reason,
            category,
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}

impl WireDetails for JobFailureCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "standalone_job"
            && version == 1
            && matches!(
                reason,
                "request"
                    | "profile"
                    | "unsupported_profile"
                    | "instruction"
                    | "catalog"
                    | "model"
                    | "worker"
                    | "result_unconfirmed"
                    | "cancelled"
                    | "missing_result"
                    | "deadline_exceeded"
                    | "host_stopped"
                    | "cleanup_unconfirmed"
            )
    }
}

impl ProjectFor<Storage> for JobFailureCause {
    type Authorization = ();
    type Details = Self;
    fn disclose(&self, _: &FailureFacts<Self>, _: &()) -> Result<Disclosure<Self>, ContractError> {
        Ok(Disclosure::new(*self, self.explanation()))
    }
}

/// Supplied by the authenticated query boundary, not read from saved attachments.
#[derive(Debug, Clone, Copy)]
pub struct JobFailureVisibility {
    pub can_view_cause: bool,
}

macro_rules! query_projection {
    ($audience:ty) => {
        impl ProjectFor<$audience> for JobFailureCause {
            type Authorization = JobFailureVisibility;
            type Details = Self;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                current: &JobFailureVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !current.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                // Rebuild explanation/context locally. Never copy received prose or context.
                Ok(Disclosure::new(*self, self.explanation()))
            }
        }
    };
}
query_projection!(Browser);
query_projection!(Tool);
query_projection!(InternalTransfer);

/// Attachment-read status only. Absence certifies no cause, including on legacy success.
/// None of these observations changes the Job result or permits another execution.
pub enum JobFailureObservation<A> {
    LegacyWithoutFacts,
    Known(ProjectedFailure<A, JobFailureCause>),
    Unsupported,
    Corrupt,
    NotVisible,
}

pub struct JobFailureObservations<A> {
    pub primary: JobFailureObservation<A>,
    pub post_result: JobFailureObservation<A>,
    pub result_receipt: JobFailureObservation<A>,
    pub cleanup: JobFailureObservation<A>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StoredJobFailures {
    pub primary: Option<StoredFailure>,
    pub post_result: Option<StoredFailure>,
    pub result_receipt: Option<StoredFailure>,
    pub cleanup: Option<StoredFailure>,
}

impl StoredJobFailures {
    pub(crate) fn project<A: Audience>(
        &self,
        current: &JobFailureVisibility,
    ) -> JobFailureObservations<A>
    where
        JobFailureCause:
            ProjectFor<A, Authorization = JobFailureVisibility, Details = JobFailureCause>,
    {
        fn one<A: Audience>(
            stored: Option<&StoredFailure>,
            current: &JobFailureVisibility,
        ) -> JobFailureObservation<A>
        where
            JobFailureCause:
                ProjectFor<A, Authorization = JobFailureVisibility, Details = JobFailureCause>,
        {
            use error_contract::compatibility::StoredFailureRead;
            // Fail closed before inspecting retained facts for a denied current viewer.
            if !current.can_view_cause {
                return JobFailureObservation::NotVisible;
            }
            match StoredFailure::read_optional::<JobFailureCause>(stored) {
                StoredFailureRead::LegacyWithoutFacts => JobFailureObservation::LegacyWithoutFacts,
                StoredFailureRead::Structured(DecodeFailure::Unsupported(_)) => {
                    JobFailureObservation::Unsupported
                }
                StoredFailureRead::Structured(DecodeFailure::Corrupt(_)) => {
                    JobFailureObservation::Corrupt
                }
                StoredFailureRead::Structured(DecodeFailure::Known(_)) => {
                    match stored
                        .expect("known attachment")
                        .project_for::<A, JobFailureCause>(current)
                    {
                        Ok(projected) => JobFailureObservation::Known(projected),
                        Err(_) => JobFailureObservation::NotVisible,
                    }
                }
            }
        }
        JobFailureObservations {
            primary: one(self.primary.as_ref(), current),
            post_result: one(self.post_result.as_ref(), current),
            result_receipt: one(self.result_receipt.as_ref(), current),
            cleanup: one(self.cleanup.as_ref(), current),
        }
    }
}

/// Writer-side observations for one settlement. Separate slots preserve successful
/// result acceptance and never promote a receipt/cleanup problem to execution failure.
#[derive(Default)]
pub(crate) struct SettlementFailures {
    pub primary: Option<JobFailureCause>,
    pub post_result: Option<JobFailureCause>,
    pub result_receipt: Option<JobFailureCause>,
    pub cleanup: Option<JobFailureCause>,
}

pub(super) const SCHEMA: &str = "
ALTER TABLE job_attempts ADD COLUMN primary_failure_json TEXT;
ALTER TABLE job_attempts ADD COLUMN post_result_failure_json TEXT;
ALTER TABLE job_attempts ADD COLUMN result_receipt_failure_json TEXT;
ALTER TABLE job_attempts ADD COLUMN cleanup_failure_json TEXT;
";
