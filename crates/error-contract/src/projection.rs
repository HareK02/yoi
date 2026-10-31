//! Domain allowlists are the only path from internal facts to encodable values.
use std::marker::PhantomData;

use crate::{codec::FailureEnvelope, *};

mod sealed {
    pub trait Sealed {}
}
pub trait Audience: sealed::Sealed {}
macro_rules! audiences {
    ($($name:ident),*) => { $(
        #[derive(Debug)] pub struct $name;
        impl sealed::Sealed for $name {}
        impl Audience for $name {}
    )* };
}
audiences!(Browser, Tool, InternalTransfer, Storage, Telemetry);

/// Domain-owned allowlisted output. Context is selected too; it is not copied implicitly.
pub struct Disclosure<D> {
    pub details: D,
    pub explanation: String,
    pub context: FailureContext,
}
impl<D> Disclosure<D> {
    pub fn new(details: D, explanation: impl Into<String>) -> Self {
        Self {
            details,
            explanation: explanation.into(),
            context: FailureContext::default(),
        }
    }
}

/// Implement separately for each audience. The boundary supplies *current* authorization
/// and resource visibility, including when projecting a previously stored/received value.
/// This trait does not authenticate a caller or turn an audience name into authority.
pub trait ProjectFor<A: Audience>: FailureCause + Sized {
    type Authorization: ?Sized;
    type Details: WireDetails;
    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        authorization: &Self::Authorization,
    ) -> Result<Disclosure<Self::Details>, ContractError>;
}

/// No Deserialize/Serialize implementation: only projection creates this trust marker.
///
/// ```compile_fail
/// use error_contract::{Browser, Storage, ProjectedFailure};
/// fn public_only<D>(_: &ProjectedFailure<Browser, D>) {}
/// fn cannot_relabel<D>(stored: &ProjectedFailure<Storage, D>) { public_only(stored); }
/// ```
///
/// ```compile_fail
/// use error_contract::{Browser, ProjectedFailure};
/// let _: ProjectedFailure<Browser, String> = serde_json::from_str("{}").unwrap();
/// ```
///
/// ```compile_fail
/// use error_contract::{Browser, Budget, ReceivedFailure, WireDetails, encode_projected_failure};
/// fn received_is_not_projected<D: WireDetails>(received: &ReceivedFailure<D>) {
///     encode_projected_failure::<Browser, D>(received, Budget::new(1024, 1024));
/// }
/// ```
pub struct ProjectedFailure<A, D> {
    pub(crate) envelope: FailureEnvelope<D>,
    pub(crate) audience: PhantomData<A>,
}
impl<A: Audience, D: FailureCause> ProjectedFailure<A, D> {
    pub fn details(&self) -> &D {
        &self.envelope.details
    }
    pub fn identity(&self) -> FailureIdentity {
        self.envelope.details.identity()
    }
    pub fn explanation(&self) -> &str {
        &self.envelope.explanation
    }
    pub fn context(&self) -> &FailureContext {
        &self.envelope.context
    }
}

pub fn project_failure<A: Audience, D: ProjectFor<A>>(
    facts: &FailureFacts<D>,
    authorization: &D::Authorization,
) -> Result<ProjectedFailure<A, D::Details>, ContractError> {
    validate_failure(facts)?;
    let disclosure = facts.cause().disclose(facts, authorization)?;
    disclosure.details.validate()?;
    let identity = disclosure.details.identity();
    // A domain can remove optional details, not silently replace a known cause.
    if identity != facts.cause().identity() {
        return Err(ContractError::InvalidDetails);
    }
    validate_explanation(&disclosure.explanation)?;
    disclosure.context.validate()?;
    Ok(ProjectedFailure {
        envelope: FailureEnvelope {
            version: SCHEMA_VERSION,
            required_extensions: Vec::new(),
            domain: identity.domain.into(),
            reason: identity.reason.into(),
            category: identity.category,
            details_version: identity.details_version,
            details: disclosure.details,
            explanation: disclosure.explanation,
            context: disclosure.context,
        },
        audience: PhantomData,
    })
}

/// Compose an allowlisted cause and impact without truncating either required part.
/// Domains select the translated fragments; consumers must not classify from this prose.
pub fn compose_explanation(cause: &str, impact: &str) -> Result<String, ContractError> {
    validate_explanation(cause)?;
    validate_explanation(impact)?;
    let explanation = format!("{impact}: {cause}");
    validate_explanation(&explanation)?;
    Ok(explanation)
}

/// Safe local presentation when no supported cause can be disclosed. This is not a
/// replacement domain failure or permission to forward unsupported payload bytes.
#[derive(Debug)]
pub struct UnavailablePresentation {
    pub detail: &'static str,
    pub stage: &'static str,
    pub correlation: Option<CorrelationRef>,
}
pub fn unavailable_presentation(
    stage: &'static str,
    correlation: Option<CorrelationRef>,
) -> Result<UnavailablePresentation, ContractError> {
    validate_identifier(stage)?;
    if let Some(reference) = &correlation {
        validate_identifier(reference.as_str())?;
    }
    Ok(UnavailablePresentation {
        detail: "Failure details unavailable.",
        stage,
        correlation,
    })
}

/// Safe fields for an existing tracing sink. No implicit recording at wrappers.
#[derive(Debug, PartialEq, Eq)]
pub struct ObservationFields<'a> {
    pub domain: &'static str,
    pub reason: &'static str,
    pub operation: Option<&'a str>,
    pub stage: Option<&'a str>,
    pub correlation: Option<&'a str>,
}
pub fn observation_fields<D: FailureCause>(
    failure: &ProjectedFailure<Telemetry, D>,
) -> ObservationFields<'_> {
    let identity = failure.identity();
    let operation = failure.context().operations.last();
    ObservationFields {
        domain: identity.domain,
        reason: identity.reason,
        operation: operation.map(|op| op.operation.as_str()),
        stage: operation.map(|op| op.stage.as_str()),
        correlation: failure
            .context()
            .correlation
            .as_ref()
            .map(CorrelationRef::as_str),
    }
}
