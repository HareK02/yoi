//! Tool result projection is independent of dispatch and of a domain's operation state.
//!
//! Callers project with current authority before entering this adapter. The existing
//! domain result is attached without interpreting its prose or authorizing a retry.
//! Structured details follow normal history pruning; the concrete explanation remains
//! in the summary. Neither telemetry nor notification delivery participates in encoding.

use std::io::Write;

use agen::tool::{ToolError, ToolOutput};
use error_contract::{Budget, ContractError, ProjectedFailure, Tool, WireDetails};
use serde::{Deserialize, Serialize};

/// A receive/transport failure does not attest to a remote execution failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolFailureBoundary {
    Execution,
    Observation,
}

#[derive(Serialize, schemars::JsonSchema)]
struct FailureReport<R, D> {
    boundary: ToolFailureBoundary,
    #[schemars(with = "FailureWireSchema<D>")]
    failure: serde_json::Value,
    result: R,
    #[serde(skip)]
    #[schemars(skip)]
    marker: std::marker::PhantomData<D>,
}

struct FailureWireSchema<D>(std::marker::PhantomData<D>);
impl<D: schemars::JsonSchema> schemars::JsonSchema for FailureWireSchema<D> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("ToolFailure_{}", D::schema_name()).into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        error_contract::schema::failure_subschema::<D>(generator)
    }
}

/// The Tool's existing result schema embeds the core cause schema and its public result DTO.
/// This does not validate, authorize, or infer the operation outcome from received JSON.
pub fn failure_report_schema<D: schemars::JsonSchema, R: schemars::JsonSchema>() -> schemars::Schema
{
    schemars::schema_for!(FailureReport<R, D>)
}

/// Retain one projected cause and an already-public domain result in the normal error output.
///
/// `result` must be the domain's current-authority public DTO, not internal state or
/// raw provider text. Only the failure projection supplies the explanation. Failure
/// to encode is a presentation failure, never a successful operation or a retry grant.
/// The enclosing Tool result budget must accommodate the failure and required result.
pub fn execution_failure<D: WireDetails, R: Serialize>(
    failure: &ProjectedFailure<Tool, D>,
    boundary: ToolFailureBoundary,
    result: &R,
    output_budget: usize,
) -> ToolError {
    let output = match encode_report(failure, boundary, result, output_budget) {
        Ok(content) => ToolOutput {
            summary: failure.explanation().to_owned(),
            content: Some(content),
            attachments: Vec::new(),
        },
        Err(_) => ToolOutput {
            // Keep the already-disclosed cause even when the required result will not fit.
            summary: format!("{} Failure details unavailable.", failure.explanation()),
            content: None,
            attachments: Vec::new(),
        },
    };
    ToolError::ExecutionFailure(output)
}

fn encode_report<D: WireDetails, R: Serialize>(
    failure: &ProjectedFailure<Tool, D>,
    boundary: ToolFailureBoundary,
    result: &R,
    output_budget: usize,
) -> Result<String, ContractError> {
    let encoded = error_contract::encode_projected_failure(
        failure,
        Budget::new(output_budget, output_budget).with_depth(
            error_contract::MAX_JSON_DEPTH,
            error_contract::MAX_JSON_DEPTH - 1,
        ),
    )?;
    let report = FailureReport {
        boundary,
        // These bytes came from the bounded projected encoder, not an untrusted response.
        failure: serde_json::from_slice(encoded.as_bytes()).map_err(|_| ContractError::Encoding)?,
        result,
        marker: std::marker::PhantomData::<D>,
    };
    let mut writer = ReportWriter {
        bytes: Vec::new(),
        limit: output_budget,
    };
    serde_json::to_writer(&mut writer, &report).map_err(|_| ContractError::Encoding)?;
    error_contract::codec::check_json_bounds(
        &writer.bytes,
        Budget::new(output_budget, output_budget),
    )?;
    String::from_utf8(writer.bytes).map_err(|_| ContractError::Encoding)
}

// Bounds the enclosing DTO as well as the cause. Required fields are never truncated
// into invalid JSON, and serialization failure cannot recursively invoke a formatter.
struct ReportWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl Write for ReportWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("Tool failure output budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::tool::{ToolResult, ToolResultDisposition};
    use error_contract::test_support::{
        CurrentAccess, FIXTURE_SECRETS, Language, PrivateSource, RequiredOperation, SubjectDenied,
        SubjectError,
    };
    use error_contract::{DecodeFailure, FailureFacts, project_failure};

    fn projected() -> ProjectedFailure<Tool, SubjectDenied> {
        project_failure(
            &FailureFacts::new(SubjectError {
                details: SubjectDenied {
                    required_operation: RequiredOperation::SessionRecord,
                    subject_ref: Some("subject-1".into()),
                },
                source: PrivateSource,
            })
            .unwrap(),
            &CurrentAccess {
                can_view_resource: false,
                language: Language::English,
            },
        )
        .unwrap()
    }

    #[test]
    fn projected_tool_error_preserves_cause_result_and_error_disposition() {
        let failure = projected();
        let error = execution_failure(
            &failure,
            ToolFailureBoundary::Execution,
            &serde_json::json!({"state":"rejected", "request_id":"request-1"}),
            64 * 1024,
        );
        let result = ToolResult::from_error("call-1", error);
        assert_eq!(result.disposition, ToolResultDisposition::Error);
        assert!(result.is_error);
        assert_eq!(result.summary, failure.explanation());
        let wire = serde_json::to_string(&result).unwrap();
        for secret in FIXTURE_SECRETS {
            assert!(!wire.contains(secret));
        }
        assert!(!wire.contains("subject-1"));
        let reopened: ToolResult = serde_json::from_str(&wire).unwrap();
        let content: serde_json::Value =
            serde_json::from_str(reopened.content.as_ref().unwrap()).unwrap();
        assert_eq!(content["result"]["request_id"], "request-1");
        assert_eq!(content["boundary"], "execution");
        let encoded = serde_json::to_vec(&content["failure"]).unwrap();
        let DecodeFailure::Known(cause) =
            error_contract::decode_failure::<SubjectDenied>(&encoded, Budget::new(65536, 65536))
        else {
            panic!("cause must roundtrip")
        };
        assert_eq!(
            cause.facts().cause().required_operation,
            RequiredOperation::SessionRecord
        );
    }

    #[test]
    fn output_budget_failure_keeps_error_and_explanation_without_partial_json() {
        let failure = projected();
        let result = ToolResult::from_error(
            "call-1",
            execution_failure(
                &failure,
                ToolFailureBoundary::Observation,
                &serde_json::json!({"state":"unknown"}),
                1,
            ),
        );
        assert!(result.is_error);
        assert!(result.summary.starts_with(failure.explanation()));
        assert!(result.content.is_none());
    }

    #[test]
    fn generated_report_schema_uses_the_core_failure_and_domain_result() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct PublicResult {
            request_id: String,
        }
        let schema = failure_report_schema::<SubjectDenied, PublicResult>();
        let validator = jsonschema::validator_for(schema.as_value()).unwrap();
        let result = ToolResult::from_error(
            "call",
            execution_failure(
                &projected(),
                ToolFailureBoundary::Execution,
                &PublicResult {
                    request_id: "request-1".into(),
                },
                65536,
            ),
        );
        let content: serde_json::Value =
            serde_json::from_str(result.content.as_ref().unwrap()).unwrap();
        assert!(validator.is_valid(&content));
        let mut invalid = content;
        invalid["failure"]["details"] = serde_json::json!({"unrelated":true});
        assert!(!validator.is_valid(&invalid));
    }

    #[test]
    fn enclosing_depth_and_escaped_byte_budgets_do_not_emit_partial_results() {
        let failure = projected();
        let mut nested = serde_json::json!("value");
        for _ in 0..error_contract::MAX_JSON_DEPTH {
            nested = serde_json::json!([nested]);
        }
        let too_deep = ToolResult::from_error(
            "call",
            execution_failure(&failure, ToolFailureBoundary::Execution, &nested, 65536),
        );
        assert!(too_deep.content.is_none());
        let escaped = serde_json::json!({"safe": "\"\\".repeat(1000)});
        let full = ToolResult::from_error(
            "call",
            execution_failure(&failure, ToolFailureBoundary::Execution, &escaped, 65536),
        );
        let byte_limit = full.content.as_ref().unwrap().len() - 1;
        let limited = ToolResult::from_error(
            "call",
            execution_failure(
                &failure,
                ToolFailureBoundary::Execution,
                &escaped,
                byte_limit,
            ),
        );
        assert!(limited.content.is_none());
        assert!(limited.is_error);
    }

    #[test]
    fn observation_failure_preserves_unknown_instead_of_asserting_remote_rejection() {
        let result = ToolResult::from_error(
            "call-1",
            execution_failure(
                &projected(),
                ToolFailureBoundary::Observation,
                &serde_json::json!({"disposition":"unknown"}),
                65536,
            ),
        );
        let content: serde_json::Value =
            serde_json::from_str(result.content.as_ref().unwrap()).unwrap();
        assert_eq!(content["boundary"], "observation");
        assert_eq!(content["result"]["disposition"], "unknown");
    }
}
