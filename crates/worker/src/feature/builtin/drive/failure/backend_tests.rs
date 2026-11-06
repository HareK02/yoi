//! Register ONLY as a child of `drive::backend::tests` (the module sourced from
//! `drive/backend_tests.rs`):
//! `#[path = "failure/backend_tests.rs"] mod failure_projection;`
//!
//! Uses that fixture's existing Client, retained state and mutation assertion.
//! This is not a new mutation writer, HTTP adapter or receipt implementation.

use super::{Client, Fault, ID, SECRET, entry, mutation};
use crate::feature::builtin::drive::{
    backend::{DriveBackend, DriveError},
    failure::{
        DriveClientFailure, DriveClientFailureDetails as Details,
        DriveClientFailureOperation as Operation, DriveClientFailureReason as Reason,
        DriveClientFailureVisibility as Visibility,
    },
};
use crate::worker::WorkspaceRequestMethod;
use error_contract::{
    Browser, Budget, DecodeFailure, FailureFacts, Tool, decode_failure, encode_projected_failure,
    project_failure,
};
use server_api::DriveRequestState;

const VISIBLE: Visibility = Visibility {
    can_view_cause: true,
};

fn equal_browser_tool(failure: DriveClientFailure, reason: Reason) {
    assert_eq!(failure.details().reason, reason);
    let details = *failure.details();
    let facts = FailureFacts::new(failure).unwrap();
    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let budget = Budget::new(4096, 4096);
    let bytes = encode_projected_failure(&browser, budget).unwrap();
    assert_eq!(
        bytes.as_bytes(),
        encode_projected_failure(&tool, budget).unwrap().as_bytes(),
    );
    let DecodeFailure::Known(received) = decode_failure::<Details>(bytes.as_bytes(), budget) else {
        panic!("typed client cause must survive the shared codec");
    };
    assert_eq!(*received.facts().cause(), details);
    // Perturb presentation around an actual adapter result, not a fabricated
    // result consumer. Reprojection must restore the same typed meaning and
    // bytes without making any provider/status call (counts checked by callers).
    let original: serde_json::Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
    for prose in [
        "SUCCESS; safely resend",
        "NOT DISPATCHED; skip reconciliation",
    ] {
        let mut wire = original.clone();
        wire["explanation"] = serde_json::json!(prose);
        wire["context"]["operations"][0]["stage"] = serde_json::json!("not_dispatched");
        wire["context"]["operations"][0]["operation_ref"] = serde_json::json!("substitute-request");
        let DecodeFailure::Known(received) =
            decode_failure::<Details>(&serde_json::to_vec(&wire).unwrap(), budget)
        else {
            panic!("same typed cause with changed untrusted presentation");
        };
        assert_eq!(*received.facts().cause(), details);
        let browser = project_failure::<Browser, _>(received.facts(), &VISIBLE).unwrap();
        let tool = project_failure::<Tool, _>(received.facts(), &VISIBLE).unwrap();
        assert_eq!(
            encode_projected_failure(&browser, budget)
                .unwrap()
                .as_bytes(),
            bytes.as_bytes(),
        );
        assert_eq!(
            encode_projected_failure(&tool, budget).unwrap().as_bytes(),
            bytes.as_bytes(),
        );
    }
    let text = std::str::from_utf8(bytes.as_bytes()).unwrap();
    assert!(text.contains("workspace_drive_client"));
    assert!(!text.contains(SECRET));
    assert!(!text.contains(ID));
    for secret in ["secret-token", "/private/drive", "credential=unsafe"] {
        assert!(!text.contains(secret), "{text}");
    }
}

#[tokio::test]
async fn production_read_failures_project_only_the_existing_unavailable_variant() {
    // Different diagnostic causes really reach the same current DriveError.
    // Projection does not read that sink or invent finer wire reasons.
    for fault in [
        Fault::Communication,
        Fault::Unavailable,
        Fault::InvalidPath,
        Fault::Decode,
        Fault::Identity,
        Fault::Metadata,
        Fault::Unrecognized,
    ] {
        let client = Client::new(fault, Fault::None);
        let backend = DriveBackend::new(client.clone());
        let error = backend.metadata(&entry().entry).await.unwrap_err();
        assert!(matches!(error, DriveError::Unavailable));
        equal_browser_tool(
            DriveClientFailure::new(Operation::Metadata, error),
            Reason::Unavailable,
        );
        let state = client.state.lock().unwrap();
        assert_eq!(state.mutations, 0);
        assert_eq!(state.saved, None);
        assert_eq!(state.requests.len(), 1);
        assert_eq!(state.requests[0].method, WorkspaceRequestMethod::Get);
    }
}

#[tokio::test]
async fn production_unknown_keeps_exact_request_and_reconcile_counts_without_resending() {
    for fault in [
        Fault::Communication,
        Fault::Decode,
        Fault::Identity,
        Fault::Metadata,
        Fault::Unknown,
        Fault::Unrecognized,
    ] {
        for receipt in [
            Fault::Length,
            Fault::Incomplete,
            Fault::Identity,
            Fault::Decode,
        ] {
            let client = Client::new(fault, receipt);
            let backend = DriveBackend::new(client.clone());
            let request = mutation();
            let operation = Operation::from(&request.mutation);
            assert_eq!(operation, Operation::UpdateText);
            let error = backend.mutate(request).await.unwrap_err();
            assert!(matches!(&error, DriveError::OutcomeUnknown(id) if id == ID));
            let failure = DriveClientFailure::new(operation, error);
            assert!(matches!(failure.source_error(), DriveError::OutcomeUnknown(id) if id == ID));
            // Recover the SAME typed identity before projecting; never derive one from
            // a cause category, correlation, explanation or reason string.
            let error = failure.into_source();
            let DriveError::OutcomeUnknown(original_id) = &error else {
                panic!("unknown result and original identity must remain available");
            };
            assert_eq!(original_id, ID);
            {
                let state = client.state.lock().unwrap();
                assert_eq!(state.mutations, 1);
                assert_eq!(state.saved.as_deref(), Some(SECRET));
                assert_eq!(state.requests.len(), 2);
                assert_eq!(state.requests[0].method, WorkspaceRequestMethod::Post);
                assert_eq!(state.requests[0].path, "/api/w/workspace/drive/mutate");
                assert_eq!(state.requests[1].method, WorkspaceRequestMethod::Get);
                assert_eq!(
                    state.requests[1].path,
                    format!("/api/w/workspace/drive/requests/{ID}"),
                );
            }
            equal_browser_tool(
                DriveClientFailure::new(operation, error.clone()),
                Reason::OutcomeUnknown,
            );
            // Explicitly reread the existing original request only; never resend.
            let status = backend.status(original_id).await;
            match receipt {
                Fault::Length => {
                    let snapshot = status.unwrap();
                    assert_eq!(snapshot.request_id, ID);
                    assert_eq!(snapshot.state, DriveRequestState::Uncommitted);
                    assert_eq!(snapshot.response, None);
                }
                Fault::Incomplete => {
                    let snapshot = status.unwrap();
                    assert_eq!(snapshot.request_id, ID);
                    assert_eq!(snapshot.state, DriveRequestState::Committed);
                    assert_eq!(snapshot.response, None);
                }
                Fault::Identity | Fault::Decode => {
                    assert!(matches!(status, Err(DriveError::Unavailable)));
                }
                _ => unreachable!("closed test cases"),
            }
            // Neither an uncommitted snapshot nor an incomplete committed receipt
            // rewrites the original Unknown. No new state machine is introduced.
            assert!(matches!(error, DriveError::OutcomeUnknown(ref id) if id == ID));
            let state = client.state.lock().unwrap();
            assert_eq!(state.mutations, 1);
            assert_eq!(state.saved.as_deref(), Some(SECRET));
            assert_eq!(state.requests.len(), 3);
            assert_eq!(state.requests[2].method, WorkspaceRequestMethod::Get);
            assert_eq!(
                state.requests[2].path,
                format!("/api/w/workspace/drive/requests/{ID}"),
            );
            assert!(state.binary.is_empty());
        }
    }
}

#[tokio::test]
async fn production_conflict_remains_distinct_from_unknown_and_adds_no_inquiry() {
    let client = Client::new(Fault::Rejected, Fault::None);
    let backend = DriveBackend::new(client.clone());
    let error = backend.mutate(mutation()).await.unwrap_err();
    assert!(matches!(error, DriveError::Conflict));
    equal_browser_tool(
        DriveClientFailure::new(Operation::UpdateText, error),
        Reason::Conflict,
    );
    let state = client.state.lock().unwrap();
    assert_eq!(state.mutations, 0);
    assert_eq!(state.saved, None);
    assert_eq!(state.requests.len(), 1);
    assert_eq!(state.requests[0].method, WorkspaceRequestMethod::Post);
}

#[tokio::test]
async fn production_valid_receipt_still_reconciles_to_success_with_one_mutation() {
    let client = Client::new(Fault::Decode, Fault::None);
    let backend = DriveBackend::new(client.clone());
    let result = backend.mutate(mutation()).await.unwrap();
    // A failure projection must not replace the existing successful result.
    assert_eq!(result.request_id, ID);
    assert_eq!(result.entry, Some(entry()));
    let state = client.state.lock().unwrap();
    assert_eq!(state.mutations, 1);
    assert_eq!(state.saved.as_deref(), Some(SECRET));
    assert_eq!(state.requests.len(), 2);
    assert_eq!(
        state.requests[1].path,
        format!("/api/w/workspace/drive/requests/{ID}"),
    );
}
