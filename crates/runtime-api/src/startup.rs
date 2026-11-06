//! Reusable startup HTTP boundary, independent of an HTTP framework.
//!
//! `WorkerCreateFailure` retains the execution owner's attachment and independent
//! observations. It is not another durable operation record and grants no retry rights.
//! Only `ProblemResponse::from_projected` encodes public responses. The public DTO is
//! schema/reader data, **not** a disclosure capability. Received explanations, labels,
//! diagnostic strings and attachment bytes are never copied to a public response.

//! ## Startup boundary adapter
//!
//! The `startup` module is framework-independent. `WorkerCreateFailure::new` wraps the
//! existing `WorkerStartupFailure` without replacing its primary attachment, secondary
//! cleanup/persistence facts, resources or IO classification. `StartupOperationFacts`
//! requires callers to supply creation acceptance and execution outcome explicitly;
//! there is no HTTP-to-outcome conversion, default rejection or retry authorization.
//! These observations are **not** a second durable result authority.
//!
//! `StartupCause` is an untagged concrete union of the adopted Session and Runtime
//! causes and the enum-only `StartupBoundaryCause`. The existing `session_registration`
//! and `worker_startup` identities and details remain unchanged. Boundary observations
//! use `startup_boundary` and distinguish timeout, transport, decode, invalid response,
//! size limit, unsupported version/reason and finalize failure. The cause does not carry
//! raw error text. Finalization stage and compensation are separate from that cause and
//! from acceptance/execution. Completed compensation does not establish non-execution.
//!
//! ### Boundary integration API
//!
//! 1. Supply `StartupOperationFacts { acceptance, execution, finalize, backend_compensation, correlation }`
//!    from actual owner observations. On response loss, preserve `Unknown` unless
//!    separate authoritative evidence establishes another outcome.
//! 2. Build `WorkerCreateFailure::new(startup, operation)` or `::from_cause(cause, operation)`.
//!    `startup()`, `operation()` and `into_parts()` support non-HTTP callers.
//! 3. Call `failure.problem(StartupBoundary::Browser, &current_visibility, instance)`.
//!    The resulting `startup::ProblemResponse` provides `status`, `content_type`,
//!    `body()` and `into_body()`. The response body includes the core `yoi_failure`
//!    envelope plus `yoi_startup: StartupProblemFacts`. The latter never contains an
//!    attachment. `StartupProblemDetails` is the public `schemars` source DTO; parsing
//!    that DTO alone grants no permission to disclose it.
//! 4. Runtime response adapters select `StartupBoundary::Runtime`. Both boundaries
//!    deliberately emit the **minimum public-safe schema**, using a fresh Browser/public
//!    projection; Runtime does not relabel InternalTransfer output as Browser output.
//!    Session disclosure requires an explicit current `SessionFailureVisibility` under
//!    authenticated Runtime scope, never a received/public label.
//!    `ProblemResponse::from_projected` supports callers already holding a
//!    `ProjectedFailure<Browser, StartupCause>` and typed facts.
//! 5. Remote clients call `decode_startup_problem(bytes, status, content_type, sender_boundary)`.
//!    Its result is `Known(WorkerCreateFailure)`, `UnsupportedVersion`,
//!    `UnsupportedReason`, `Invalid` or `TooLarge`. The boundary argument describes
//!    the **sender**, so a Runtime Subject denial decodes as 403 and is later projected
//!    as 409 at the Browser boundary. Do not classify the Browser response by forwarding
//!    that 403. Unknown results never expose raw retained input.
//!
//! The normalizer reuses core byte/depth checks (32 KiB / 16 levels), envelope validation,
//! identity validation and Problem Details consistency checks. It distinguishes unknown
//! versions, details versions and required extensions from unknown reasons/domains using
//! validated metadata, without interpreting unknown details. Known input is projected
//! again through the actual concrete domain's InternalTransfer allowlist. Wire detail,
//! explanation, context diagnostics and references are not forwarded. A later Browser
//! projection applies current Session visibility again. Missing acceptance/execution is
//! invalid, not an implicit `Unknown` or `Rejected`.
//!
//! ### Classification and disclosure
//!
//! | Cause | Browser | Runtime |
//! | --- | --- | --- |
//! | Session operator denied | 403 | 403 |
//! | Session Subject/unknown-principal denied | 409 | 403 |
//! | Session attribution missing / binding mismatch | 409 | 409 |
//! | Session transport / unknown / invalid response | 502 | 502 |
//! | Boundary timeout | 504 | 504 |
//! | Boundary transport / decode / invalid / oversized / unsupported | 502 | 502 |
//! | Finalize failure | 500 | 500 |
//! | Runtime empty Segment / missing prompt | 400 | 400 |
//! | Runtime metadata pending/missing/inactive / manifest missing | 409 | 409 |
//! | Runtime allocation failure | 503 | 503 |
//! | Other closed Runtime startup reasons | 500 | 500 |
//!
//! Classification uses typed causes only, never severity, diagnostic order, explanation,
//! HTTP hints, compensation or resources. Browser Session explanations explicitly name
//! `session.record`, the principal class and the Worker impact. Domain projectors retain
//! only their allowlisted context and remove operation references; received text is not
//! a source of public explanations.
//!
//! `yoi_startup` retains independently: acceptance, execution, optional finalization stage
//! and compensation, optional **Backend compensation** even without finalization, optional
//! safe correlation, all secondary facts, resource facts, persistence and path-free IO
//! kind. Backend compensation is not inferred from Runtime cleanup or finalize facts.
//! Only these closed enum facts are copied into
//! this extension. `StartupCorrelation::new` accepts at most 128 ASCII letters/digits,
//! periods, underscores and hyphens. It is an opaque lookup token, not a credential or
//! operation identity. Callers must choose an actual safe token (lexical validation cannot
//! identify a secret) and a safe `instance` URI-reference, not a raw upstream URL.
//!
//! ### RuntimeApiError HTTP hooks
//!
//! `RuntimeApiError::with_create_failure(failure)` retains local typed facts in a
//! `serde(skip)` field. `create_failure()` and `into_create_failure()` expose them to
//! internal callers. The Runtime API's `HttpError::custom_response` resolves a fresh
//! public-safe projection for the Runtime boundary; invalid/unavailable projection uses
//! the core static Problem Details fallback, never the legacy attachment serialization.
//! `with_create_failure` and Runtime HTTP serving are trusted local adapters invoked
//! **after current Runtime scope authentication/authorization**; this minimum schema
//! policy does not authorize Browser disclosure. Ordinary errors without startup facts
//! retain their original JSON/status behavior.
//!
//! `RuntimeApi` opts into `client_error_hook`. The generated client passes the actual
//! HTTP status, unique Content-Type and bounded bytes to `RuntimeApiError`'s
//! `HttpClientError` implementation. Known Problems retain normalized typed facts.
//! Unsupported version and unsupported reason become distinct local boundary observations
//! with acceptance/execution `Unknown`; malformed/oversized Problems also remain local
//! observations rather than reports of remote execution failure. Collection failures
//! outside the hook remain the macro's precise `ClientFailure` values and carry no
//! execution verdict.
//!
//! Legacy `application/json` errors use a narrow typed schema under the same 32 KiB byte
//! limit; top-level/detail extension fields are rejected, and nested adopted facts are
//! parsed as typed values rather than allocated as arbitrary JSON trees. Valid startup attachments are projected again into InternalTransfer with
//! acceptance/execution left `Unknown` because the legacy shape does not report them.
//! A legacy unsupported attachment exposes no safe version/reason metadata, so it is
//! classified locally as invalid response rather than guessed from opaque bytes.
//! Non-startup legacy errors retain their code/message and actual HTTP status.
//!
//! The core currently exposes encoded responses, not a generic projected Problem DTO
//! constructor. The extension encoder therefore reuses core URI/classification/envelope
//! validation and reads that bounded core DTO before attaching typed facts. A core
//! `to_problem_dto` primitive could remove this in-memory round trip; implementing a
//! second URI/envelope encoder here would be less maintainable and is intentionally
//! avoided.
//!
//! ### Investigation / recording ownership
//!
//! This module does not write metadata, log, reconcile, retry or compensate. The execution
//! owner remains responsible for persistence and actual resource cleanup; the parent
//! boundary records one safe correlation at its existing operation/result/log boundary.
//! Use that correlation to locate the existing owner result, compare acceptance and
//! execution separately, then inspect finalize/compensation and resource/persistence
//! facts. Do not infer a new operation is safe from timeout, failed persistence or
//! completed compensation. Do not log raw unsupported HTTP bodies or attachments.
//!
//! `WorkerCreateFailure::telemetry()` returns a `ProjectedFailure<Telemetry, StartupCause>`
//! for `error_contract::observation_fields`, not a log write. The projector invokes the
//! concrete domain allowlist, selects fixed operation/stage labels and drops all received
//! operation references and attachment correlation. It uses only the typed operation
//! correlation as a correlation reference, never as an operation identity. Backend callers
//! should replace any remote correlation with their fresh local reservation-bound token
//! before recording at the Backend responsibility boundary.
//!
//! The shared `tests/fixtures/` inputs include a constructor-generated denial and
//! exact Browser HTTP bodies captured by workspace-server's registered
//! `session_registration_connected` tests (Subject denial and committed-registration
//! response loss). `startup_contract` checks consumer normalization and preservation
//! through the HTTP-independent storage codec. These captured operation facts are not
//! synthesized from HTTP status. Fixture changes must be intentional source changes;
//! tests never rewrite fixtures or select behavior through ambient environment flags.
//! The `startup_http` integration tests exercise actual generated RuntimeApi clients
//! against HTTP responses, and the normal generated-router round trip covers startup
//! Problem responses as well as legacy errors. Actual Subject authorization, Runtime
//! execution/persistence and Backend integration remain the parent Ticket's responsibility;
//! these contract tests do not substitute for those connected checks.
//!

pub(crate) mod http;
mod storage;

pub use storage::{DecodeStoredWorkerCreateFailure, decode_stored_worker_create_failure};

use error_contract::codec::FailureEnvelope;
use error_contract::problem::{
    ApiBoundary, HttpFailureStatus, ProblemClass, ProblemDetails, decode_problem_details,
    to_problem_details,
};
use error_contract::{
    Browser, Budget, Category, ContractError, DecodeFailure, Disclosure, FailureCause,
    FailureFacts, FailureIdentity, InternalTransfer, ProjectFor, ProjectedFailure, Telemetry,
    WireDetails, project_failure,
};
pub use protocol::session_failure::SessionFailureVisibility;
use protocol::session_failure::{
    SessionPrincipal, SessionRegistrationCause, SessionRegistrationReason,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    RuntimeStartupCause, RuntimeStartupReason, StartupFailureAttachment, StartupIoKind,
    StartupPersistenceOutcome, StartupResourceFacts, StartupSecondaryFailure, WorkerStartupFailure,
};

pub use error_contract::problem::PROBLEM_CONTENT_TYPE;
pub const MAX_STARTUP_PROBLEM_BYTES: usize = error_contract::MAX_FAILURE_BYTES;
pub const STARTUP_FACTS_VERSION: u16 = 1;

fn budget() -> Budget {
    Budget::new(MAX_STARTUP_PROBLEM_BYTES, MAX_STARTUP_PROBLEM_BYTES)
}

/// Concrete domain union: serialization is the original domain details object,
/// not an additional tagged wrapper. Envelope domain/reason identity is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum StartupCause {
    Session(SessionRegistrationCause),
    Runtime(RuntimeStartupCause),
    Boundary(StartupBoundaryCause),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StartupBoundaryReason {
    Timeout,
    Transport,
    Decode,
    InvalidResponse,
    TooLarge,
    UnsupportedVersion,
    UnsupportedReason,
    FinalizeFailed,
}

/// An observation at the receiving/finalizing boundary, not a reported Runtime
/// execution result. It deliberately cannot hold raw transport or error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartupBoundaryCause {
    pub reason: StartupBoundaryReason,
}

impl FailureCause for StartupBoundaryCause {
    fn identity(&self) -> FailureIdentity {
        use StartupBoundaryReason::*;
        let reason = match self.reason {
            Timeout => "timeout",
            Transport => "transport",
            Decode => "decode",
            InvalidResponse => "invalid_response",
            TooLarge => "too_large",
            UnsupportedVersion => "unsupported_version",
            UnsupportedReason => "unsupported_reason",
            FinalizeFailed => "finalize_failed",
        };
        FailureIdentity {
            domain: "startup_boundary",
            reason,
            category: if self.reason == FinalizeFailed {
                Category::Internal
            } else {
                Category::Dependency
            },
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}

impl WireDetails for StartupBoundaryCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "startup_boundary"
            && version == 1
            && matches!(
                reason,
                "timeout"
                    | "transport"
                    | "decode"
                    | "invalid_response"
                    | "too_large"
                    | "unsupported_version"
                    | "unsupported_reason"
                    | "finalize_failed"
            )
    }
}

impl FailureCause for StartupCause {
    fn identity(&self) -> FailureIdentity {
        match self {
            Self::Session(cause) => cause.identity(),
            Self::Runtime(cause) => cause.identity(),
            Self::Boundary(cause) => cause.identity(),
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        match self {
            Self::Session(cause) => cause.validate(),
            Self::Runtime(cause) => cause.validate(),
            Self::Boundary(cause) => cause.validate(),
        }
    }
}

impl WireDetails for StartupCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        SessionRegistrationCause::supports(domain, reason, version)
            || RuntimeStartupCause::supports(domain, reason, version)
            || StartupBoundaryCause::supports(domain, reason, version)
    }
}

// Reconstruct concrete domain facts, then invoke that domain's actual allowlist.
// No received diagnostic/cause/correlation context is implicitly redisclosed.
fn domain_facts<D: FailureCause>(
    cause: D,
    facts: &FailureFacts<StartupCause>,
) -> Result<FailureFacts<D>, ContractError> {
    let mut concrete = FailureFacts::new(cause)?;
    for operation in &facts.context().operations {
        concrete.with_context(operation.clone())?;
    }
    Ok(concrete)
}

fn wrap<D>(
    disclosure: Disclosure<D>,
    wrap_details: fn(D) -> StartupCause,
    explanation: String,
) -> Disclosure<StartupCause> {
    Disclosure {
        details: wrap_details(disclosure.details),
        explanation,
        context: disclosure.context,
    }
}

fn session_explanation(cause: &SessionRegistrationCause) -> String {
    let principal = match cause.principal {
        SessionPrincipal::Operator => "Operator",
        SessionPrincipal::Subject => "Subject",
        SessionPrincipal::Unknown => "Unknown principal",
    };
    let why = match cause.reason {
        SessionRegistrationReason::PermissionDenied => {
            "is not authorized for required operation session.record"
        }
        SessionRegistrationReason::AttributionMissing => {
            "attribution is missing for required operation session.record"
        }
        SessionRegistrationReason::BindingMismatch => {
            "binding does not match for required operation session.record"
        }
        SessionRegistrationReason::Transport => {
            "could not be confirmed because the session.record transport failed"
        }
        SessionRegistrationReason::OutcomeUnknown => "has an unknown session.record outcome",
        SessionRegistrationReason::InvalidResponse => "received an invalid session.record response",
    };
    format!(
        "Worker creation could not complete Session registration: {principal} {why}. Creation acceptance and execution outcome are reported separately."
    )
}

fn runtime_explanation(cause: &RuntimeStartupCause) -> &'static str {
    use RuntimeStartupReason::*;
    match cause.reason {
        StartupFailed => "Worker startup failed at the Runtime.",
        SegmentEmpty => "Worker startup requires a nonempty Segment.",
        SystemPromptMissing => "Worker startup requires a system prompt.",
        MetadataPending => "Worker startup metadata is pending reconciliation.",
        MetadataMissing => "Worker startup metadata is missing.",
        MetadataInactive => "Worker startup metadata is inactive.",
        ManifestMissing => "Worker startup manifest is missing.",
        AllocationFailed => "Worker startup resource allocation failed.",
        SessionReadFailed => "Worker startup could not read the Session.",
        MetadataReadFailed => "Worker startup could not read metadata.",
        ProviderFailed => "Worker startup provider initialization failed.",
        AttributionFailed => "Worker startup attribution failed.",
        FeatureFailed => "Worker startup feature initialization failed.",
    }
}

fn boundary_explanation(cause: &StartupBoundaryCause) -> &'static str {
    use StartupBoundaryReason::*;
    match cause.reason {
        Timeout => "Worker creation response timed out; this does not establish non-execution.",
        Transport => {
            "Worker creation response transport failed; this does not establish non-execution."
        }
        Decode => {
            "Worker creation response could not be decoded; execution outcome remains independently reported."
        }
        InvalidResponse => {
            "Worker creation returned an invalid response; execution outcome remains independently reported."
        }
        TooLarge => {
            "Worker creation response exceeded the supported size limit; execution outcome remains independently reported."
        }
        UnsupportedVersion => {
            "Worker creation response uses an unsupported contract version; execution outcome remains independently reported."
        }
        UnsupportedReason => {
            "Worker creation response contains an unsupported failure reason; execution outcome remains independently reported."
        }
        FinalizeFailed => {
            "Worker creation finalization failed; acceptance, execution and compensation are reported separately."
        }
    }
}

impl ProjectFor<Browser> for StartupCause {
    type Authorization = SessionFailureVisibility;
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        visibility: &SessionFailureVisibility,
    ) -> Result<Disclosure<Self>, ContractError> {
        match self {
            Self::Session(cause) => {
                let concrete = domain_facts(*cause, facts)?;
                let disclosed = <SessionRegistrationCause as ProjectFor<Browser>>::disclose(
                    cause, &concrete, visibility,
                )?;
                Ok(wrap(disclosed, Self::Session, session_explanation(cause)))
            }
            Self::Runtime(cause) => {
                let concrete = domain_facts(cause.clone(), facts)?;
                let disclosed =
                    <RuntimeStartupCause as ProjectFor<Browser>>::disclose(cause, &concrete, &())?;
                Ok(wrap(
                    disclosed,
                    Self::Runtime,
                    runtime_explanation(cause).into(),
                ))
            }
            Self::Boundary(cause) => Ok(Disclosure::new(self.clone(), boundary_explanation(cause))),
        }
    }
}

impl ProjectFor<InternalTransfer> for StartupCause {
    type Authorization = ();
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        _: &(),
    ) -> Result<Disclosure<Self>, ContractError> {
        match self {
            Self::Session(cause) => {
                let concrete = domain_facts(*cause, facts)?;
                let disclosed =
                    <SessionRegistrationCause as ProjectFor<InternalTransfer>>::disclose(
                        cause,
                        &concrete,
                        &(),
                    )?;
                let explanation = session_explanation(cause);
                Ok(wrap(disclosed, Self::Session, explanation))
            }
            Self::Runtime(cause) => {
                let concrete = domain_facts(cause.clone(), facts)?;
                let disclosed = <RuntimeStartupCause as ProjectFor<InternalTransfer>>::disclose(
                    cause,
                    &concrete,
                    &(),
                )?;
                Ok(wrap(
                    disclosed,
                    Self::Runtime,
                    runtime_explanation(cause).into(),
                ))
            }
            Self::Boundary(cause) => Ok(Disclosure::new(self.clone(), boundary_explanation(cause))),
        }
    }
}

impl ProjectFor<Telemetry> for StartupCause {
    type Authorization = ();
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        _: &(),
    ) -> Result<Disclosure<Self>, ContractError> {
        // Reuse concrete domain allowlists, not arbitrary received context.
        let mut disclosure = <Self as ProjectFor<InternalTransfer>>::disclose(self, facts, &())?;
        let stage = match self {
            Self::Session(_) => "binding",
            Self::Runtime(_) => "runtime",
            Self::Boundary(StartupBoundaryCause {
                reason: StartupBoundaryReason::FinalizeFailed,
            }) => "finalize",
            Self::Boundary(_) => "response",
        };
        // Telemetry selects one typed responsibility context, not a received
        // context ordering or arbitrary operation identity.
        disclosure.context.operations.clear();
        disclosure
            .context
            .operations
            .push(error_contract::OperationContext::new(
                "worker_startup",
                stage,
            )?);
        if let Some(correlation) = &facts.context().correlation {
            let safe = StartupCorrelation::new(correlation.as_str())?;
            disclosure.context.correlation =
                Some(error_contract::CorrelationRef::new(safe.as_str())?);
        }
        Ok(disclosure)
    }
}

/// Both ends select a boundary explicitly. Never forwards an inner HTTP status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupBoundary {
    Browser,
    Runtime,
}

impl ApiBoundary<StartupCause> for StartupBoundary {
    fn classify(&self, cause: &StartupCause) -> ProblemClass {
        use HttpFailureStatus::*;
        let status = match (self, cause) {
            (Self::Browser, StartupCause::Session(cause)) => {
                match (cause.reason, cause.principal) {
                    (SessionRegistrationReason::PermissionDenied, SessionPrincipal::Operator) => {
                        Forbidden
                    }
                    (
                        SessionRegistrationReason::PermissionDenied
                        | SessionRegistrationReason::AttributionMissing
                        | SessionRegistrationReason::BindingMismatch,
                        _,
                    ) => Conflict,
                    (
                        SessionRegistrationReason::Transport
                        | SessionRegistrationReason::OutcomeUnknown
                        | SessionRegistrationReason::InvalidResponse,
                        _,
                    ) => BadGateway,
                }
            }
            (Self::Runtime, StartupCause::Session(cause)) => match cause.reason {
                SessionRegistrationReason::PermissionDenied => Forbidden,
                SessionRegistrationReason::AttributionMissing
                | SessionRegistrationReason::BindingMismatch => Conflict,
                SessionRegistrationReason::Transport
                | SessionRegistrationReason::OutcomeUnknown
                | SessionRegistrationReason::InvalidResponse => BadGateway,
            },
            (_, StartupCause::Runtime(cause)) => match cause.reason {
                RuntimeStartupReason::SegmentEmpty | RuntimeStartupReason::SystemPromptMissing => {
                    BadRequest
                }
                RuntimeStartupReason::MetadataPending
                | RuntimeStartupReason::MetadataMissing
                | RuntimeStartupReason::MetadataInactive
                | RuntimeStartupReason::ManifestMissing => Conflict,
                RuntimeStartupReason::AllocationFailed => Unavailable,
                RuntimeStartupReason::StartupFailed
                | RuntimeStartupReason::SessionReadFailed
                | RuntimeStartupReason::MetadataReadFailed
                | RuntimeStartupReason::ProviderFailed
                | RuntimeStartupReason::AttributionFailed
                | RuntimeStartupReason::FeatureFailed => Internal,
            },
            (_, StartupCause::Boundary(cause)) => match cause.reason {
                StartupBoundaryReason::Timeout => GatewayTimeout,
                StartupBoundaryReason::FinalizeFailed => Internal,
                StartupBoundaryReason::Transport
                | StartupBoundaryReason::Decode
                | StartupBoundaryReason::InvalidResponse
                | StartupBoundaryReason::TooLarge
                | StartupBoundaryReason::UnsupportedVersion
                | StartupBoundaryReason::UnsupportedReason => BadGateway,
            },
        };
        let (type_uri, title) = match status {
            Forbidden => (
                "urn:yoi:problem:authorization-denied",
                "Authorization denied",
            ),
            Conflict => ("urn:yoi:problem:conflict", "Operation conflict"),
            BadRequest => ("urn:yoi:problem:invalid-input", "Invalid input"),
            GatewayTimeout => (
                "urn:yoi:problem:startup-timeout",
                "Startup response timeout",
            ),
            BadGateway => ("urn:yoi:problem:dependency", "Dependency failure"),
            Unavailable => (
                "urn:yoi:problem:startup-unavailable",
                "Startup resources unavailable",
            ),
            _ => ("urn:yoi:problem:startup-failed", "Worker startup failed"),
        };
        ProblemClass {
            status,
            type_uri,
            title,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CreationAcceptance {
    Rejected,
    Accepted,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Accepted,
    Busy,
    Rejected,
    Errored,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FinalizeStage {
    WorkerIdentity,
    WorkspaceApiBinding,
    CreateReservation,
    WorkerRegistry,
    WorkdirAttachment,
    WorkdirRegistry,
    TicketAssignmentBind,
    TicketAssignmentCurrent,
    TicketStateAccept,
    SessionRegistration,
    InitialInput,
    Publication,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompensationOutcome {
    Completed,
    Residual,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinalizeFacts {
    pub stage: FinalizeStage,
    pub compensation: CompensationOutcome,
}

/// Opaque, caller-selected lookup token, not an operation identity or capability.
/// No whitespace, URI/path/query syntax or control characters are accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct StartupCorrelation(
    #[schemars(length(min = 1, max = 128), regex(pattern = "^[A-Za-z0-9._-]+$"))] String,
);

impl StartupCorrelation {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(ContractError::Invalid);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for StartupCorrelation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Explicit owner observations. There is deliberately no Default and no conversion
/// from HTTP status, retry hints, resource facts or failure reason into these fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartupOperationFacts {
    pub acceptance: CreationAcceptance,
    pub execution: ExecutionOutcome,
    pub finalize: Option<FinalizeFacts>,
    /// Backend compensation can occur before/without finalization; never infer
    /// this observation from Runtime cleanup, resources or finalize compensation.
    pub backend_compensation: Option<CompensationOutcome>,
    pub correlation: Option<StartupCorrelation>,
}

/// The extension contains only typed allowlisted observations; never the attachment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartupProblemFacts {
    #[schemars(range(min = 1, max = 1))]
    pub version: u16,
    pub operation: StartupOperationFacts,
    #[schemars(length(max = 8))]
    pub secondary: Vec<StartupSecondaryFailure>,
    pub resources: Option<StartupResourceFacts>,
    /// Session metadata persistence, independent of the Backend reservation result.
    pub persistence: Option<StartupPersistenceOutcome>,
    /// Backend-owned result write only; never evidence of Session metadata persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_result_persistence: Option<StartupPersistenceOutcome>,
    pub io_kind: Option<StartupIoKind>,
}

impl StartupProblemFacts {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.version != STARTUP_FACTS_VERSION
            || self.secondary.len() > error_contract::MAX_DIAGNOSTICS
        {
            return Err(ContractError::InvalidDetails);
        }
        Ok(())
    }
}

/// Public generator/reader DTO. Deserialization does not grant disclosure authority.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StartupProblemDetails {
    #[serde(rename = "type")]
    #[schemars(length(max = 2048))]
    pub type_uri: String,
    #[schemars(length(min = 1, max = 2048))]
    pub title: String,
    #[schemars(range(min = 400, max = 599))]
    pub status: u16,
    #[schemars(length(min = 1, max = 2048))]
    pub detail: String,
    #[schemars(length(max = 2048))]
    pub instance: String,
    #[schemars(schema_with = "startup_envelope_schema")]
    pub yoi_failure: FailureEnvelope<StartupCause>,
    pub yoi_startup: StartupProblemFacts,
}

// The envelope's existing domain is the wire discriminator. Describing only the
// inner untagged implementation union would lose this external contract and make
// generated clients guess between overlapping reason fields.
fn startup_envelope_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let domains = [
        (
            "session_registration",
            FailureEnvelope::<SessionRegistrationCause>::json_schema(generator),
        ),
        (
            "worker_startup",
            FailureEnvelope::<RuntimeStartupCause>::json_schema(generator),
        ),
        (
            "startup_boundary",
            FailureEnvelope::<StartupBoundaryCause>::json_schema(generator),
        ),
    ];
    let branches = domains
        .into_iter()
        .map(|(domain, mut schema)| {
            let properties = schema
                .as_object_mut()
                .and_then(|schema| schema.get_mut("properties"))
                .and_then(serde_json::Value::as_object_mut)
                .expect("FailureEnvelope is an object with typed properties");
            properties.insert(
                "domain".into(),
                serde_json::json!({"type": "string", "const": domain}),
            );
            serde_json::Value::from(schema)
        })
        .collect::<Vec<_>>();
    schemars::json_schema!({ "oneOf": branches })
}

impl api_macros::openapi::OpenApiSchema for StartupProblemDetails {}

/// HTTP-independent failure retaining the exact original startup attachment.
/// Not serializable to prevent accidentally publishing internal attachments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerCreateFailure {
    startup: WorkerStartupFailure,
    operation: StartupOperationFacts,
    backend_result_persistence: Option<StartupPersistenceOutcome>,
}

impl WorkerCreateFailure {
    pub fn new(
        startup: WorkerStartupFailure,
        operation: StartupOperationFacts,
    ) -> Result<Self, ContractError> {
        let failure = Self {
            startup,
            operation,
            backend_result_persistence: None,
        };
        failure.public_facts().validate()?;
        Ok(failure)
    }

    pub fn from_cause(
        cause: StartupCause,
        operation: StartupOperationFacts,
    ) -> Result<Self, ContractError> {
        let facts = FailureFacts::new(cause)?;
        let projected = project_failure::<InternalTransfer, _>(&facts, &())?;
        Self::new(
            WorkerStartupFailure {
                primary: StartupFailureAttachment::from_projected(&projected)?,
                secondary: Vec::new(),
                resources: None,
                persistence: None,
                io_kind: None,
            },
            operation,
        )
    }

    pub fn startup(&self) -> &WorkerStartupFailure {
        &self.startup
    }
    pub fn operation(&self) -> &StartupOperationFacts {
        &self.operation
    }
    /// Backend reservation result persistence only, not Session metadata persistence.
    pub fn backend_result_persistence(&self) -> Option<StartupPersistenceOutcome> {
        self.backend_result_persistence
    }

    /// Set a local write observation. A Backend receiver must clear a remote marker
    /// before its own save; an encoder writes the current marker without inferring it.
    pub fn with_backend_result_persistence(
        mut self,
        outcome: Option<StartupPersistenceOutcome>,
    ) -> Self {
        self.backend_result_persistence = outcome;
        self
    }

    /// Replace boundary observations without discarding Backend persistence evidence.
    pub fn with_operation(mut self, operation: StartupOperationFacts) -> Self {
        self.operation = operation;
        self
    }

    /// Legacy two-part decomposition discards Backend result persistence. Extract
    /// `backend_result_persistence()` first and restore it after `new`, or prefer
    /// `with_operation` when only the operation observations need replacement.
    pub fn into_parts(self) -> (WorkerStartupFailure, StartupOperationFacts) {
        (self.startup, self.operation)
    }

    pub fn public_facts(&self) -> StartupProblemFacts {
        StartupProblemFacts {
            version: STARTUP_FACTS_VERSION,
            operation: self.operation.clone(),
            secondary: self.startup.secondary.clone(),
            resources: self.startup.resources,
            persistence: self.startup.persistence,
            backend_result_persistence: self.backend_result_persistence,
            io_kind: self.startup.io_kind,
        }
    }

    /// Safe observation fields for an existing sink. This method never logs.
    /// The operation correlation is only a lookup token, never an operation identity.
    /// Backend callers should replace a remote correlation with their fresh local
    /// token before observing at their own responsibility boundary.
    pub fn telemetry(&self) -> Result<ProjectedFailure<Telemetry, StartupCause>, ContractError> {
        let received = match self.startup.primary.decode::<StartupCause>() {
            DecodeFailure::Known(received) => received,
            _ => return Err(ContractError::ProjectionUnavailable),
        };
        let mut facts = FailureFacts::new(received.facts().cause().clone())?;
        for context in &received.facts().context().operations {
            facts.with_context(context.clone())?;
        }
        // Never copy correlation from the attachment's untrusted context.
        if let Some(correlation) = &self.operation.correlation {
            facts.set_correlation(error_contract::CorrelationRef::new(correlation.as_str())?)?;
        }
        project_failure::<Telemetry, _>(&facts, &())
    }

    /// Both boundaries deliberately send the minimum public-safe cause schema.
    /// Runtime callers must resolve visibility under their authenticated scope;
    /// this is a fresh Browser/public-schema projection, never an audience relabel.
    pub fn problem(
        &self,
        boundary: StartupBoundary,
        visibility: &SessionFailureVisibility,
        instance: &str,
    ) -> Result<ProblemResponse, ContractError> {
        let projected = self
            .startup
            .primary
            .project_for::<Browser, StartupCause>(visibility)?;
        ProblemResponse::from_projected(&projected, boundary, instance, self.public_facts())
    }
}

/// Encoded public response. The body cannot be replaced with raw attachment data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemResponse {
    pub status: u16,
    pub content_type: &'static str,
    body: Vec<u8>,
}

impl ProblemResponse {
    pub fn from_projected(
        projected: &ProjectedFailure<Browser, StartupCause>,
        boundary: StartupBoundary,
        instance: &str,
        facts: StartupProblemFacts,
    ) -> Result<Self, ContractError> {
        facts.validate()?;
        // Reuse the core encoder for URI, classification and envelope validation.
        let core = to_problem_details(projected, &boundary, instance, budget())?;
        let dto: ProblemDetails<StartupCause> =
            serde_json::from_slice(core.body()).map_err(|_| ContractError::Encoding)?;
        let dto = StartupProblemDetails {
            type_uri: dto.type_uri,
            title: dto.title,
            status: dto.status,
            detail: dto.detail,
            instance: dto.instance,
            yoi_failure: dto.yoi_failure,
            yoi_startup: facts,
        };
        let body = serde_json::to_vec(&dto).map_err(|_| ContractError::Encoding)?;
        if body.len() > MAX_STARTUP_PROBLEM_BYTES {
            return Err(ContractError::TooLarge);
        }
        // The complete extension is included in the same byte/depth budget as the core.
        if let DecodeFailure::Corrupt(error) = decode_problem_details::<StartupCause>(
            &body,
            core.status,
            core.content_type,
            &boundary,
            budget(),
        ) {
            return Err(error);
        }
        Ok(Self {
            status: core.status,
            content_type: core.content_type,
            body,
        })
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
    pub fn into_body(self) -> Vec<u8> {
        self.body
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeStartupProblem {
    Known(WorkerCreateFailure),
    UnsupportedVersion,
    UnsupportedReason,
    Invalid,
    TooLarge,
}

// Original-byte reader for safe unsupported metadata, including duplicate rejection.
#[derive(Deserialize)]
struct Metadata {
    yoi_failure: EnvelopeMetadata,
    yoi_startup: ExtensionMetadata,
}
#[derive(Deserialize)]
struct EnvelopeMetadata {
    version: u16,
    details_version: u16,
    required_extensions: Vec<String>,
    // Read all core fields from original bytes even on unsupported envelopes.
    // The core validates their values; this pass additionally rejects duplicate
    // metadata that could otherwise be collapsed by its tolerant JSON reader.
    #[serde(rename = "domain")]
    _domain: String,
    #[serde(rename = "reason")]
    _reason: String,
    #[serde(rename = "category")]
    _category: String,
    #[serde(rename = "details")]
    _details: serde::de::IgnoredAny,
    #[serde(rename = "explanation")]
    _explanation: String,
    #[serde(rename = "context")]
    _context: error_contract::FailureContext,
}
#[derive(Deserialize)]
struct ExtensionMetadata {
    version: u16,
}

/// Bounded receiver normalization. Never derives acceptance/execution from HTTP,
/// returns raw unsupported bytes, trusts wire explanations, or retries anything.
/// `boundary` is the sender's known API boundary, not the next public boundary.
/// A known result must still be projected with current visibility before publication.
pub fn decode_startup_problem(
    bytes: &[u8],
    status: u16,
    content_type: &str,
    boundary: StartupBoundary,
) -> DecodeStartupProblem {
    use DecodeStartupProblem::*;
    // Core performs byte/depth guards before JSON allocation and validates standard
    // Problem Details coherence, envelope identity and domain details on known data.
    let decoded =
        decode_problem_details::<StartupCause>(bytes, status, content_type, &boundary, budget());
    match decoded {
        DecodeFailure::Corrupt(ContractError::TooLarge) => TooLarge,
        DecodeFailure::Corrupt(_) => Invalid,
        decoded => {
            let Ok(metadata) = serde_json::from_slice::<Metadata>(bytes) else {
                return Invalid;
            };
            if metadata.yoi_startup.version == 0 {
                return Invalid;
            }
            if metadata.yoi_failure.version != error_contract::SCHEMA_VERSION
                || metadata.yoi_failure.details_version != 1
                || !metadata.yoi_failure.required_extensions.is_empty()
                || metadata.yoi_startup.version != STARTUP_FACTS_VERSION
            {
                return UnsupportedVersion;
            }
            let received = match decoded {
                DecodeFailure::Known(received) => received,
                DecodeFailure::Unsupported(_) => return UnsupportedReason,
                DecodeFailure::Corrupt(_) => unreachable!(),
            };
            // Parse the original bytes, not Value, so duplicate typed fields fail.
            let Ok(dto) = serde_json::from_slice::<StartupProblemDetails>(bytes) else {
                return Invalid;
            };
            if dto.yoi_startup.validate().is_err() {
                return Invalid;
            }
            let Ok(projected) = project_failure::<InternalTransfer, _>(received.facts(), &())
            else {
                return Invalid;
            };
            let Ok(primary) = StartupFailureAttachment::from_projected(&projected) else {
                return Invalid;
            };
            let extension = dto.yoi_startup;
            match WorkerCreateFailure::new(
                WorkerStartupFailure {
                    primary,
                    secondary: extension.secondary,
                    resources: extension.resources,
                    persistence: extension.persistence,
                    io_kind: extension.io_kind,
                },
                extension.operation,
            ) {
                Ok(failure) => Known(
                    failure.with_backend_result_persistence(extension.backend_result_persistence),
                ),
                Err(_) => Invalid,
            }
        }
    }
}
