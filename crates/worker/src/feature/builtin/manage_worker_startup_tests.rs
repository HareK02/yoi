//! The final Spawn consumer must keep Startup causes, not just intermediate HTTP bytes.
use super::*;
use runtime_api::startup::{StartupCause, StartupProblemFacts};
use tools::failure::ToolFailureBoundary;

const DENIED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../runtime-api/tests/fixtures/startup-browser-session-record-denied.problem.json"
));
const LOST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../runtime-api/tests/fixtures/startup-browser-registration-response-lost.problem.json"
));

#[test]
fn spawn_startup_session_cause_and_independent_facts_survive_without_received_prose() {
    for fixture in [DENIED, LOST] {
        let mut body: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let expected: StartupProblemFacts =
            serde_json::from_value(body["yoi_startup"].clone()).unwrap();
        let expected_details = body["yoi_failure"]["details"].clone();
        body["detail"] = "private-body-canary /private/path".into();
        body["yoi_failure"]["explanation"] = body["detail"].clone();
        let error = tool_output(
            WorkerOperation::Spawn,
            WorkspaceResponse {
                status: body["status"].as_u64().unwrap() as u16,
                content_type: Some("application/problem+json".into()),
                body: body.to_string(),
            },
        )
        .unwrap_err();
        let ToolError::ExecutionFailure(output) = error else {
            panic!("expected structured execution failure")
        };
        assert!(!format!("{output:?}").contains("private-body-canary"));
        assert!(output.summary.contains("Session registration"));
        let report: serde_json::Value =
            serde_json::from_str(output.content.as_ref().unwrap()).unwrap();
        assert_eq!(
            report["boundary"],
            serde_json::to_value(ToolFailureBoundary::Observation).unwrap()
        );
        assert_eq!(report["failure"]["details"], expected_details);
        assert_eq!(report["failure"]["domain"], "session_registration");
        assert_eq!(
            report["result"]["startup"],
            serde_json::to_value(expected).unwrap()
        );
        let error_contract::DecodeFailure::Known(_) = error_contract::decode_failure::<StartupCause>(
            &serde_json::to_vec(&report["failure"]).unwrap(),
            error_contract::Budget::new(65536, 65536),
        ) else {
            panic!("Tool failure must be a valid known Startup cause")
        };
    }
}

#[test]
fn spawn_startup_receiver_rejects_wrong_media_status_unknown_and_corrupt_without_disclosure() {
    let base: serde_json::Value = serde_json::from_str(DENIED).unwrap();
    let status = base["status"].as_u64().unwrap() as u16;
    let mut unsupported = base.clone();
    unsupported["yoi_failure"]["details_version"] = 999.into();
    let mut corrupt = base.clone();
    corrupt["yoi_startup"]["operation"]["execution"] = "invented".into();
    for (body, status, content_type) in [
        (base.to_string(), status, None),
        (base.to_string(), status, Some("application/json".into())),
        (
            base.to_string(),
            503,
            Some("application/problem+json".into()),
        ),
        (
            unsupported.to_string(),
            status,
            Some("application/problem+json".into()),
        ),
        (
            corrupt.to_string(),
            status,
            Some("application/problem+json".into()),
        ),
    ] {
        let error = tool_output(
            WorkerOperation::Spawn,
            WorkspaceResponse {
                status,
                content_type,
                body,
            },
        )
        .unwrap_err();
        let ToolError::ExecutionFailure(output) = error else {
            panic!("expected observation failure")
        };
        let report: serde_json::Value =
            serde_json::from_str(output.content.as_ref().unwrap()).unwrap();
        assert_eq!(report["failure"]["domain"], "runtime_control");
        assert_eq!(report["failure"]["reason"], "result_unavailable");
        assert_eq!(
            report["boundary"],
            serde_json::to_value(ToolFailureBoundary::Observation).unwrap()
        );
        assert!(report["result"].get("startup").is_none());
    }
    let problem = server_api::control_failure::control_problem(
        protocol::control_failure::RuntimeControlCause::AuthorizationFailed,
    )
    .unwrap();
    let error = tool_output(
        WorkerOperation::Spawn,
        WorkspaceResponse {
            status: problem.status,
            content_type: Some(problem.content_type.into()),
            body: String::from_utf8(problem.body.to_vec()).unwrap(),
        },
    )
    .unwrap_err();
    let ToolError::ExecutionFailure(output) = error else {
        panic!("expected known Control failure")
    };
    let report: serde_json::Value = serde_json::from_str(output.content.as_ref().unwrap()).unwrap();
    assert_eq!(report["failure"]["reason"], "authorization_failed");
}

#[test]
fn startup_tool_projection_uses_domain_visibility_and_handles_runtime_and_boundary_causes() {
    use error_contract::{FailureFacts, Tool, project_failure};
    use protocol::session_failure::{
        SessionFailureVisibility, SessionPrincipal, SessionRegistrationCause,
    };
    use runtime_api::startup::{StartupBoundaryCause, StartupBoundaryReason};
    let visibility = SessionFailureVisibility {
        can_view_cause: false,
    };
    let session = FailureFacts::new(StartupCause::Session(SessionRegistrationCause::denied(
        SessionPrincipal::Subject,
    )))
    .unwrap();
    assert!(project_failure::<Tool, _>(&session, &visibility).is_err());
    for cause in [
        StartupCause::Runtime(protocol::startup_failure::RuntimeStartupCause {
            stage: protocol::startup_failure::StartupStage::Binding,
            reason: protocol::startup_failure::RuntimeStartupReason::SessionReadFailed,
        }),
        StartupCause::Boundary(StartupBoundaryCause {
            reason: StartupBoundaryReason::Transport,
        }),
    ] {
        let facts = FailureFacts::new(cause).unwrap();
        assert!(project_failure::<Tool, _>(&facts, &visibility).is_ok());
    }
}
