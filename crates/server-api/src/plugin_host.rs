//! Runtime-to-owner Host protocol. Authentication is transport-bound, never a DTO claim.
use crate::PluginHttpMethod;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginHostRequest {
    /// Fresh export/restore admission only; issues no invocation or effect identity.
    Prepare {
        plan_id: String,
        installation_id: String,
        tools: Vec<String>,
    },
    Begin {
        plan_id: String,
        installation_id: String,
        consumer: PluginConsumer,
    },
    /// Irreversible cancellation of this authenticated logical invocation.
    Cancel {
        invocation_id: String,
    },
    Authorize {
        invocation_id: String,
        operation: PluginHostOperation,
    },
    Open {
        invocation_id: String,
        slot: String,
    },
    Send {
        invocation_id: String,
        slot: String,
        request: PluginHttpRequest,
    },
    Log {
        invocation_id: String,
        entry: PluginLogEntry,
    },
    ResolveTicket {
        invocation_id: String,
        ticket_ref: String,
        access: crate::PluginTicketAccess,
    },
    ReadTicket {
        invocation_id: String,
        ticket_id: String,
    },
    DataGet {
        invocation_id: String,
        ticket_id: String,
        address: crate::PluginDataAddress,
    },
    DataMutate {
        invocation_id: String,
        ticket_id: String,
        address: crate::PluginDataAddress,
        operation_key: String,
        mutation: crate::PluginDataMutation,
    },
    DataList {
        invocation_id: String,
        ticket_id: String,
        collection: crate::PluginDataCollection,
        cursor: Option<String>,
        limit: u32,
    },
    DataLookupMutation {
        invocation_id: String,
        ticket_id: String,
        address: crate::PluginDataAddress,
        operation_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginConsumer {
    ToolCall {
        tool_call_id: String,
        tool_name: String,
    },
    /// Reserved consumer identity; this does not enable service/event delivery.
    Consumer {
        consumer_id: String,
        event_id: String,
        export_name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginHostOperation {
    Admit,
    Publish,
    Open { slot: String },
    Send { slot: String },
    Log,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginHttpRequest {
    pub method: PluginHttpMethod,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout_ms: u32,
    pub effect_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginHostResponse {
    Invocation {
        invocation_id: String,
        #[schemars(range(min = 0, max = 9_007_199_254_740_991_i64))]
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        expires_at_ms: i64,
    },
    Authorized,
    Opened,
    Response {
        response: PluginHttpResponse,
    },
    Logged,
    TicketResolved {
        ticket_id: String,
    },
    TicketRead {
        snapshot: crate::PluginTicketSnapshot,
    },
    DataCell {
        cell: crate::PluginDataCell,
    },
    DataMutated {
        receipt: crate::PluginDataReceipt,
    },
    DataListed {
        page: crate::PluginDataPage,
    },
    DataMutationLookup {
        outcome: crate::PluginDataMutationLookup,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginLogEntry {
    pub level: PluginLogLevel,
    pub event_code: String,
    pub message: String,
    pub fields: Vec<PluginLogField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginLogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginLogField {
    pub name: String,
    pub value: PluginLogScalar,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginLogScalar {
    Text(String),
    Boolean(bool),
    Signed(
        #[serde(with = "decimal_integer")]
        #[schemars(with = "String")]
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        i64,
    ),
    Unsigned(
        #[serde(with = "decimal_integer")]
        #[schemars(with = "String")]
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        u64,
    ),
    Number(f64),
}

// WIT scalars retain their full 64-bit range without JSON/TypeScript precision loss.
mod decimal_integer {
    pub fn serialize<T: std::fmt::Display, S: serde::Serializer>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }
    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
    where
        T: std::str::FromStr + std::fmt::Display,
        D: serde::Deserializer<'de>,
    {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        let value: T = text
            .parse()
            .map_err(|_| serde::de::Error::custom("invalid decimal integer"))?;
        if value.to_string() != text {
            return Err(serde::de::Error::custom("non-canonical decimal integer"));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginHostErrorCode {
    InvalidInput,
    Denied,
    NotFound,
    Conflict,
    Unsupported,
    QuotaExceeded,
    DeadlineExceeded,
    Cancelled,
    Unavailable,
    Indeterminate,
    ReceiptExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginEffectObservation {
    NotDispatched,
    ResponseObserved,
    Indeterminate,
}

/// Deliberately excludes raw diagnostics, transport bodies and arbitrary messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginHostError {
    pub code: PluginHostErrorCode,
    pub effect: PluginEffectObservation,
    pub diagnostic_id: String,
}

impl PluginHostError {
    pub fn message(&self) -> &'static str {
        match self.code {
            PluginHostErrorCode::InvalidInput => "invalid plugin Host input",
            PluginHostErrorCode::Denied => "plugin Host operation denied",
            PluginHostErrorCode::NotFound => "plugin Host target not found",
            PluginHostErrorCode::Conflict => "plugin Host operation conflict",
            PluginHostErrorCode::Unsupported => "plugin Host operation unsupported",
            PluginHostErrorCode::QuotaExceeded => "plugin Host quota exceeded",
            PluginHostErrorCode::DeadlineExceeded => "plugin Host deadline exceeded",
            PluginHostErrorCode::Cancelled => "plugin Host operation cancelled",
            PluginHostErrorCode::Unavailable => "plugin Host unavailable",
            PluginHostErrorCode::Indeterminate => "plugin Host outcome indeterminate",
            PluginHostErrorCode::ReceiptExpired => "plugin Host receipt expired",
        }
    }
}
impl std::fmt::Display for PluginHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for PluginHostError {}
impl api_macros::HttpError for PluginHostError {
    fn status_code(&self) -> u16 {
        match self.code {
            PluginHostErrorCode::InvalidInput | PluginHostErrorCode::Unsupported => 400,
            PluginHostErrorCode::Denied => 403,
            PluginHostErrorCode::NotFound => 404,
            PluginHostErrorCode::Conflict
            | PluginHostErrorCode::Indeterminate
            | PluginHostErrorCode::ReceiptExpired => 409,
            PluginHostErrorCode::QuotaExceeded => 429,
            PluginHostErrorCode::DeadlineExceeded
            | PluginHostErrorCode::Cancelled
            | PluginHostErrorCode::Unavailable => 503,
        }
    }
}
impl api_macros::HttpRequestError for PluginHostError {
    fn from_request_rejection(status: u16, _message: String) -> Self {
        Self {
            code: if status == 413 {
                PluginHostErrorCode::QuotaExceeded
            } else {
                PluginHostErrorCode::InvalidInput
            },
            effect: PluginEffectObservation::NotDispatched,
            diagnostic_id: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_protocol_rejects_identity_claims_and_unknown_fields() {
        for value in [
            serde_json::json!({"kind":"begin","plan_id":"p","installation_id":"i","consumer":{"kind":"tool_call","tool_call_id":"t","tool_name":"n"},"subject":"owner"}),
            serde_json::json!({"kind":"begin","plan_id":"p","installation_id":"i","consumer":{"kind":"tool_call","tool_call_id":"t","tool_name":"n","worker_id":"w"}}),
            serde_json::json!({"kind":"authorize","invocation_id":"i","operation":{"kind":"admit","subject":"owner"}}),
            serde_json::json!({"kind":"send","invocation_id":"i","slot":"s","request":{"method":"GET","path":"/","query":[],"headers":[],"body":[],"timeout_ms":1,"effect_key":"e","url":"https://untrusted"}}),
        ] {
            assert!(serde_json::from_value::<PluginHostRequest>(value).is_err());
        }
        assert!(
            serde_json::from_value::<PluginHostResponse>(
                serde_json::json!({"kind":"authorized","subject":"owner"})
            )
            .is_err()
        );
        assert!(serde_json::from_value::<PluginHostError>(serde_json::json!({"code":"denied","effect":"not_dispatched","diagnostic_id":"d","message":"secret"})).is_err());
    }

    #[test]
    fn log_integer_wire_preserves_full_wit_range_without_json_precision_loss() {
        for scalar in [
            PluginLogScalar::Signed(i64::MIN),
            PluginLogScalar::Signed(i64::MAX),
            PluginLogScalar::Unsigned(u64::MAX),
        ] {
            let wire = serde_json::to_value(&scalar).unwrap();
            assert!(wire["value"].is_string());
            assert_eq!(
                serde_json::from_value::<PluginLogScalar>(wire).unwrap(),
                scalar
            );
        }
        for value in [
            serde_json::json!(1),
            serde_json::json!("01"),
            serde_json::json!("+1"),
            serde_json::json!("18446744073709551616"),
        ] {
            assert!(
                serde_json::from_value::<PluginLogScalar>(
                    serde_json::json!({"kind":"unsigned","value":value})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn host_route_uses_runtime_context_and_closed_error_schema() {
        let document = crate::canonical_openapi_document().unwrap();
        let value: serde_json::Value = serde_json::from_str(&document.to_json().unwrap()).unwrap();
        let route = &value["paths"]["/api/w/{workspace_id}/plugin-host"]["post"];
        for status in [200, 400, 401, 403, 404, 409, 413, 429, 503] {
            assert!(
                route["responses"][status.to_string()].is_object(),
                "{status}"
            );
        }
        assert!(
            route["security"].is_null(),
            "signed Runtime context, not human bearer/browser auth"
        );
        let params = route["parameters"].as_array().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(params[0]["name"], "workspace_id");
        let error = &value["components"]["schemas"]["PluginHostError"];
        assert_eq!(error["additionalProperties"], false);
        assert_eq!(error["properties"].as_object().unwrap().len(), 3);
    }
}
