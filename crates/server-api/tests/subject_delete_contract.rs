use serde_json::json;
use server_api::{
    SubjektivSubjectDeleteRequest, SubjektivSubjectDeleteResponse, SubjektivSubjectState,
};

#[test]
fn subject_delete_api_replaces_retire_and_requires_operation_identity() {
    let document: serde_json::Value = serde_json::from_str(
        &server_api::canonical_openapi_document()
            .unwrap()
            .to_json()
            .unwrap(),
    )
    .unwrap();
    let paths = &document["paths"];
    assert!(
        paths
            .get("/api/w/{workspace_id}/subjektiv/subjects/{subject_id}/retire")
            .is_none()
    );
    let operation = &paths["/api/w/{workspace_id}/subjektiv/subjects/{subject_id}"]["delete"];
    assert_eq!(operation["operationId"], "subjektiv_subject_delete");
    assert_eq!(operation["requestBody"]["required"], true);
    assert!(serde_json::from_value::<SubjektivSubjectDeleteRequest>(json!({})).is_err());
    assert!(
        serde_json::from_value::<SubjektivSubjectDeleteRequest>(
            json!({"operation_id":"op", "force":true})
        )
        .is_err()
    );
    for completed in [false, true] {
        let response = SubjektivSubjectDeleteResponse {
            subject_id: "subject".into(),
            operation_id: "op".into(),
            completed,
        };
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({"subject_id":"subject", "operation_id":"op", "completed":completed})
        );
    }
}

#[test]
fn old_subject_state_is_explicit_saved_compatibility_not_a_new_operation() {
    let state: SubjektivSubjectState = serde_json::from_value(json!("retired")).unwrap();
    assert_eq!(state, SubjektivSubjectState::LegacyRetired);
    assert_eq!(serde_json::to_value(state).unwrap(), json!("retired"));
}
