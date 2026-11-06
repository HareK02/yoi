//! HTTP-independent snapshot of a Backend coordinator failure. This is a codec
//! for the existing reservation result, not a separate ledger or retry authority.
use super::*;
use crate::StoredStartupFailureAttachment;
use error_contract::{Storage, validate_json_input};

const STORAGE_VERSION: u16 = 1;

// Private DTO: only the concrete failure encoder may construct a stored snapshot.
// In particular no arbitrary error/source, HTTP response or private identity fits here.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredWorkerCreateFailure {
    version: u16,
    primary: StoredStartupFailureAttachment,
    facts: StartupProblemFacts,
}

// Inspect versions from original input before attempting to understand future facts.
// IgnoredAny still checks JSON grammar without allocating arbitrary extension trees.
#[derive(Deserialize)]
struct StorageMetadata {
    version: u16,
    #[serde(rename = "primary")]
    _primary: serde::de::IgnoredAny,
    facts: ExtensionMetadata,
}

impl ProjectFor<Storage> for StartupBoundaryCause {
    type Authorization = ();
    type Details = Self;

    fn disclose(&self, _: &FailureFacts<Self>, _: &()) -> Result<Disclosure<Self>, ContractError> {
        // The boundary domain stores only its local enum, never received context.
        Ok(Disclosure::new(*self, boundary_explanation(self)))
    }
}

impl ProjectFor<Storage> for StartupCause {
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
                let disclosed = <SessionRegistrationCause as ProjectFor<Storage>>::disclose(
                    cause,
                    &concrete,
                    &(),
                )?;
                Ok(wrap(disclosed, Self::Session, session_explanation(cause)))
            }
            Self::Runtime(cause) => {
                let concrete = domain_facts(cause.clone(), facts)?;
                let disclosed =
                    <RuntimeStartupCause as ProjectFor<Storage>>::disclose(cause, &concrete, &())?;
                Ok(wrap(
                    disclosed,
                    Self::Runtime,
                    runtime_explanation(cause).into(),
                ))
            }
            Self::Boundary(cause) => {
                let concrete = domain_facts(*cause, facts)?;
                let disclosed =
                    <StartupBoundaryCause as ProjectFor<Storage>>::disclose(cause, &concrete, &())?;
                Ok(wrap(
                    disclosed,
                    Self::Boundary,
                    boundary_explanation(cause).into(),
                ))
            }
        }
    }
}

impl WorkerCreateFailure {
    /// Encode a bounded version-1 storage snapshot through each concrete domain's
    /// Storage allowlist. Never encodes or parses an HTTP body. The caller selects
    /// the Backend persistence marker (including prospective Persisted for an
    /// atomic reservation write); encoding alone never proves persistence.
    pub fn encode_storage(&self) -> Result<String, ContractError> {
        let facts = self.public_facts();
        facts.validate()?;
        let projected = self
            .startup
            .primary
            .project_for::<Storage, StartupCause>(&())?;
        let dto = StoredWorkerCreateFailure {
            version: STORAGE_VERSION,
            primary: StoredStartupFailureAttachment::from_projected(&projected)?,
            facts,
        };
        let encoded = serde_json::to_string(&dto).map_err(|_| ContractError::Encoding)?;
        // Includes escaped attachment bytes and the entire enclosing snapshot.
        validate_json_input(encoded.as_bytes(), budget())?;
        Ok(encoded)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeStoredWorkerCreateFailure {
    Known(WorkerCreateFailure),
    Unsupported,
    Corrupt(ContractError),
}

/// Read a bounded storage snapshot without an HTTP classification or boundary.
/// Unsupported/corrupt evidence never becomes a fabricated primary cause, and a
/// Known result still needs current visibility for any later public projection.
/// Missing legacy columns are represented by the store's Option, not this codec.
pub fn decode_stored_worker_create_failure(input: &str) -> DecodeStoredWorkerCreateFailure {
    match decode_checked(input) {
        Ok(value) => value,
        Err(error) => DecodeStoredWorkerCreateFailure::Corrupt(error),
    }
}

fn decode_checked(input: &str) -> Result<DecodeStoredWorkerCreateFailure, ContractError> {
    use DecodeStoredWorkerCreateFailure::*;
    // Run the shared lexical guard over the whole input before any JSON parsing.
    validate_json_input(input.as_bytes(), budget())?;
    let metadata: StorageMetadata =
        serde_json::from_str(input).map_err(|_| ContractError::Invalid)?;
    if metadata.version == 0 || metadata.facts.version == 0 {
        return Err(ContractError::Invalid);
    }
    if metadata.version != STORAGE_VERSION || metadata.facts.version != STARTUP_FACTS_VERSION {
        return Ok(Unsupported);
    }
    // Original-byte typed decoding rejects duplicates rather than collapsing a Value.
    let dto: StoredWorkerCreateFailure =
        serde_json::from_str(input).map_err(|_| ContractError::InvalidDetails)?;
    dto.facts.validate()?;
    let received = match dto.primary.decode::<StartupCause>() {
        DecodeFailure::Known(received) => received,
        DecodeFailure::Unsupported(_) => return Ok(Unsupported),
        DecodeFailure::Corrupt(error) => return Err(error),
    };
    // Deserialization is not a capability. Project again instead of relabeling or
    // retaining raw loaded attachment bytes (including injected explanations/context).
    let projected = project_failure::<Storage, _>(received.facts(), &())?;
    let primary = StartupFailureAttachment::from_projected(&projected)?;
    let facts = dto.facts;
    let failure = WorkerCreateFailure::new(
        WorkerStartupFailure {
            primary,
            secondary: facts.secondary,
            resources: facts.resources,
            persistence: facts.persistence,
            io_kind: facts.io_kind,
        },
        facts.operation,
    )?
    .with_backend_result_persistence(facts.backend_result_persistence);
    Ok(Known(failure))
}
