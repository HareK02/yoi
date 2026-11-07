//! Real adoption decisions, SQLite state and receipts; not a substitute for HTTP
//! authentication or the shared Tool/background result adapter.
use super::*;
use crate::configuration_failure::{ConfigurationFailureDetails, ConfigurationFailureVisibility};
use error_contract::{
    Browser, Budget, DecodeFailure, FailureFacts, Tool, decode_failure, encode_projected_failure,
    project_failure,
};

#[test]
fn deciding_branch_causes_keep_original_state_policy_head_and_receipts() {
    let (_dir, db) = fixture();
    let host = host(false);
    let initial = request(&db, &host, "initial", Some(A));
    let committed = adopt(&db, &host, &initial).unwrap();
    let policy = db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap();
    for (kind, reason) in [
        ("catalog", FailureReason::CatalogChanged),
        ("head", FailureReason::EditingHeadChanged),
        ("source", FailureReason::SourceContentChanged),
        ("entry", FailureReason::EntryConflict),
    ] {
        let mut candidate = request(&db, &host, kind, Some(B));
        match kind {
            "catalog" => candidate.expected_catalog_digest = "private-unobserved-catalog".into(),
            "head" => candidate.expected_head = Some("private-unobserved-head".into()),
            "source" => candidate.source.base_digest = "private-unobserved-source".into(),
            "entry" => {
                let ConfigTreeChange::Update {
                    expected_digest, ..
                } = &mut candidate.source.changes[0]
                else {
                    panic!("update fixture")
                };
                *expected_digest = "private-unobserved-entry".into();
            }
            _ => unreachable!(),
        }
        let Error::Configuration(cause) = adopt(&db, &host, &candidate).unwrap_err() else {
            panic!("the deciding branch must retain a typed configuration cause")
        };
        assert_eq!(cause.details().operation, FailureOperation::Adopt);
        assert_eq!(cause.details().reason, reason);
        if kind == "entry" {
            assert!(matches!(
                cause.source_error(),
                Some(config_source::ConfigTreeError::EntryConflict(_))
            ));
        } else {
            assert!(cause.source_error().is_none());
        }
        let facts = FailureFacts::new(cause).unwrap();
        let visible = ConfigurationFailureVisibility {
            can_view_cause: true,
        };
        let browser = project_failure::<Browser, _>(&facts, &visible).unwrap();
        let tool = project_failure::<Tool, _>(&facts, &visible).unwrap();
        let budget = Budget::new(4096, 4096);
        let bytes = encode_projected_failure(&browser, budget).unwrap();
        assert_eq!(
            bytes.as_bytes(),
            encode_projected_failure(&tool, budget).unwrap().as_bytes()
        );
        assert!(
            !std::str::from_utf8(bytes.as_bytes())
                .unwrap()
                .contains("private-")
        );
        assert!(browser.explanation().contains("source and policy adoption"));
        let DecodeFailure::Known(received) =
            decode_failure::<ConfigurationFailureDetails>(bytes.as_bytes(), budget)
        else {
            panic!("supported cause")
        };
        assert_eq!(received.facts().cause(), browser.details());
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(committed.state.clone())
        );
        assert_eq!(
            db.configuration_head(WORKSPACE, OWNER).unwrap(),
            Some(committed.history_head.clone())
        );
        assert_eq!(
            db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap(),
            policy
        );
        assert!(
            db.configuration_adoption_result(WORKSPACE, OWNER, kind)
                .unwrap()
                .is_none()
        );
        assert_eq!(adopt(&db, &host, &initial).unwrap(), committed);
    }
}

#[test]
fn request_identity_conflict_and_owner_denial_stay_typed_without_a_second_update() {
    let (_dir, db) = fixture();
    let host = host(false);
    let original = request(&db, &host, "same-request", Some(A));
    let committed = adopt(&db, &host, &original).unwrap();
    let changed = request(&db, &host, "same-request", Some(B));
    let Error::Configuration(cause) = adopt(&db, &host, &changed).unwrap_err() else {
        panic!("typed request conflict")
    };
    assert_eq!(
        cause.details().reason,
        FailureReason::RequestIdentityConflict
    );
    assert_eq!(cause.details().operation, FailureOperation::Adopt);
    let Error::Configuration(cause) = db
        .configuration_request_result(WORKSPACE, OWNER, &changed)
        .unwrap_err()
    else {
        panic!("typed reconciliation conflict")
    };
    assert_eq!(
        cause.details().reason,
        FailureReason::RequestIdentityConflict
    );
    assert_eq!(cause.details().operation, FailureOperation::Reconcile);
    let Error::Configuration(cause) = db
        .adopt_configuration(
            WORKSPACE,
            "not-the-owner",
            &changed,
            schema(),
            &host,
            None,
            |_| panic!("owner denial precedes protected validation"),
            |_| panic!("owner denial must not prepare a transfer"),
        )
        .unwrap_err()
    else {
        panic!("typed owner denial")
    };
    assert_eq!(cause.details().reason, FailureReason::OwnerRequired);
    assert_eq!(cause.details().operation, FailureOperation::Adopt);
    assert_eq!(adopt(&db, &host, &original).unwrap(), committed);
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap(),
        Some(committed.state)
    );
    assert_eq!(
        db.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(committed.history_head)
    );
}
