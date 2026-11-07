//! Post-adoption observations are not adoption receipts or execution authority.
//! Missing observations remain unknown. Querying never retries publication.
use error_contract::codec::FailureEnvelope;
use protocol::control_failure::RuntimeControlCause;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::configuration_failure::ConfigurationFailureDetails;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationPublicationStage {
    Dispatch,
    Projection,
    Runtime,
}

/// Observed means queue admission, projection availability, or Runtime acknowledgement,
/// depending on stage. It never confirms model processing or current policy application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationPublicationOutcome {
    Observed,
    Unconfirmed,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConfigurationPublicationFailure {
    Configuration {
        failure: FailureEnvelope<ConfigurationFailureDetails>,
    },
    Runtime {
        failure: FailureEnvelope<RuntimeControlCause>,
    },
    /// Retained unknown/corrupt evidence is not forwarded or replaced by a made-up cause.
    Unavailable,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPublicationObservation {
    pub stage: ConfigurationPublicationStage,
    pub runtime_id: Option<String>,
    pub projection_digest: Option<String>,
    pub outcome: ConfigurationPublicationOutcome,
    pub failure: Option<ConfigurationPublicationFailure>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPublicationResponse {
    pub workspace_id: String,
    pub operation_id: String,
    pub request_fingerprint: String,
    pub history_head: String,
    pub source_digest: String,
    pub observations: Vec<ConfigurationPublicationObservation>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPublicationLookup {
    pub publication: Option<ConfigurationPublicationResponse>,
}
impl api_macros::openapi::OpenApiSchema for ConfigurationPublicationLookup {}
impl api_macros::openapi::OpenApiSchema for ConfigurationPublicationResponse {}
