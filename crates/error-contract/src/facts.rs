use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum Category {
    Authorization,
    InvalidInput,
    Conflict,
    Dependency,
    Internal,
    Unknown,
}

/// Identity is derived from a domain variant, never supplied independently by a caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureIdentity {
    pub domain: &'static str,
    pub reason: &'static str,
    pub category: Category,
    pub details_version: u16,
}

pub trait FailureCause {
    fn identity(&self) -> FailureIdentity;
    /// Domain-owned required fields, length and collection bounds.
    fn validate(&self) -> Result<(), ContractError>;
}

/// Implement on a typed wire allowlist, not on arbitrary error strings or maps.
pub trait WireDetails: FailureCause + Serialize + serde::de::DeserializeOwned {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct OperationContext {
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub operation: String,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub stage: String,
    /// A reference, not an idempotency/retry grant. Disclosure remains domain-owned.
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub operation_ref: Option<String>,
}

impl OperationContext {
    pub fn new(
        operation: impl Into<String>,
        stage: impl Into<String>,
    ) -> Result<Self, ContractError> {
        let context = Self {
            operation: operation.into(),
            stage: stage.into(),
            operation_ref: None,
        };
        context.validate()?;
        Ok(context)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        validate_identifier(&self.operation)?;
        validate_identifier(&self.stage)?;
        if let Some(reference) = &self.operation_ref {
            validate_identifier(reference)?;
        }
        Ok(())
    }
}

/// Correlation is deliberately a distinct field/type from operation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct CorrelationRef(
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))] String,
);

impl CorrelationRef {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        validate_identifier(&value)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Only explicit causal references belong here. Cleanup/audit failures stay in domain results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct CausalLink {
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub domain: String,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub reason: String,
    pub correlation: CorrelationRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "json-schema", schemars(rename = "FailureDiagnostic"))]
pub struct Diagnostic {
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 128)))]
    pub code: String,
    #[cfg_attr(feature = "json-schema", schemars(length(min = 1, max = 2048)))]
    pub explanation: String,
}

/// Common metadata is untrusted after deserialization. No audience label confers trust.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct FailureContext {
    #[cfg_attr(feature = "json-schema", schemars(length(max = 8)))]
    pub operations: Vec<OperationContext>,
    /// Ordered immediate-to-root chain, at most four references; never recursive payloads.
    #[cfg_attr(feature = "json-schema", schemars(length(max = 4)))]
    pub causes: Vec<CausalLink>,
    #[cfg_attr(feature = "json-schema", schemars(length(max = 8)))]
    pub diagnostics: Vec<Diagnostic>,
    pub correlation: Option<CorrelationRef>,
    pub supplementary_omitted: bool,
}

impl FailureContext {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.operations.len() > MAX_CONTEXTS
            || self.causes.len() > MAX_CAUSES
            || self.diagnostics.len() > MAX_DIAGNOSTICS
        {
            return Err(ContractError::Invalid);
        }
        for op in &self.operations {
            op.validate()?;
        }
        for cause in &self.causes {
            validate_identifier(&cause.domain)?;
            validate_identifier(&cause.reason)?;
            validate_identifier(cause.correlation.as_str())?;
        }
        for diagnostic in &self.diagnostics {
            validate_identifier(&diagnostic.code)?;
            validate_explanation(&diagnostic.explanation)?;
        }
        if let Some(correlation) = &self.correlation {
            validate_identifier(correlation.as_str())?;
        }
        Ok(())
    }
}

/// Not serializable: the domain retains its concrete error and source chain.
#[derive(Debug)]
pub struct FailureFacts<D> {
    cause: D,
    context: FailureContext,
}

impl<D: FailureCause> FailureFacts<D> {
    pub fn new(cause: D) -> Result<Self, ContractError> {
        let facts = Self {
            cause,
            context: FailureContext::default(),
        };
        validate_failure(&facts)?;
        Ok(facts)
    }
    pub fn cause(&self) -> &D {
        &self.cause
    }
    pub fn context(&self) -> &FailureContext {
        &self.context
    }
    /// A failed append leaves the original facts intact.
    pub fn with_context(&mut self, context: OperationContext) -> Result<(), ContractError> {
        context.validate()?;
        if self.context.operations.len() == MAX_CONTEXTS {
            return Err(ContractError::Invalid);
        }
        self.context.operations.push(context);
        Ok(())
    }
    pub fn set_correlation(&mut self, correlation: CorrelationRef) -> Result<(), ContractError> {
        validate_identifier(correlation.as_str())?;
        self.context.correlation = Some(correlation);
        Ok(())
    }
    pub fn with_causal_link(&mut self, cause: CausalLink) -> Result<(), ContractError> {
        validate_identifier(&cause.domain)?;
        validate_identifier(&cause.reason)?;
        validate_identifier(cause.correlation.as_str())?;
        if self.context.causes.len() == MAX_CAUSES {
            return Err(ContractError::Invalid);
        }
        self.context.causes.push(cause);
        Ok(())
    }
    pub(crate) fn from_received(cause: D, context: FailureContext) -> Self {
        Self { cause, context }
    }
}

pub fn validate_failure<D: FailureCause>(facts: &FailureFacts<D>) -> Result<(), ContractError> {
    let identity = facts.cause.identity();
    validate_identifier(identity.domain)?;
    validate_identifier(identity.reason)?;
    if identity.details_version == 0 {
        return Err(ContractError::InvalidDetails);
    }
    facts.cause.validate()?;
    facts.context.validate()
}

impl<D: FailureCause> fmt::Display for FailureFacts<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let identity = self.cause.identity();
        write!(f, "{}.{}", identity.domain, identity.reason)
    }
}
impl<D: FailureCause + Error + 'static> Error for FailureFacts<D> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
