use super::{create, fixture, grant_authorizer, grant_fixture};
use crate::failure::{
    DriveFailure, DriveFailureDetails, DriveFailureOperation as Operation,
    DriveFailureReason as Reason, DriveFailureVisibility,
};
use crate::{Error, Mutation, RequestStatus, blob};
use error_contract::{
    Browser, Budget, Category, ContractError, CorrelationRef, DecodeFailure, FailureCause,
    FailureFacts, InternalTransfer, OperationContext, Storage, Telemetry, Tool, decode_failure,
    encode_projected_failure, observation_fields, project_failure,
};
use serde_json::{Value, json};

const VISIBLE: DriveFailureVisibility = DriveFailureVisibility {
    can_view_cause: true,
};

fn budget() -> Budget {
    Budget::new(4096, 4096)
}

fn equal_browser_tool(operation: Operation, source: Error, reason: Reason) -> Vec<u8> {
    let failure = DriveFailure::new(operation, source);
    assert_eq!(failure.details().operation, operation);
    assert_eq!(failure.details().reason, reason);
    assert!(
        std::error::Error::source(&failure)
            .unwrap()
            .downcast_ref::<Error>()
            .is_some(),
        "the actual Drive error must remain the local source"
    );
    let facts = FailureFacts::new(failure).unwrap();
    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let browser_wire = encode_projected_failure(&browser, budget()).unwrap();
    let tool_wire = encode_projected_failure(&tool, budget()).unwrap();
    assert_eq!(browser_wire.as_bytes(), tool_wire.as_bytes());
    let DecodeFailure::Known(received) =
        decode_failure::<DriveFailureDetails>(browser_wire.as_bytes(), budget())
    else {
        panic!("supported Drive cause must survive the shared codec");
    };
    assert_eq!(received.facts().cause(), browser.details());
    assert_eq!(received.facts().cause().identity(), browser.identity());
    assert_eq!(browser.identity().domain, "drive");
    assert_eq!(browser.identity().reason, reason.as_str());
    assert_ne!(browser.identity().category, Category::Unknown);
    assert!(browser.explanation().contains("Drive"));
    assert!(browser.explanation().contains("reported a failure"));
    browser_wire.as_bytes().to_vec()
}

#[test]
fn name_conflict_keeps_original_tree_and_receipt_through_codec_and_reopen() {
    let root_dir = tempfile::tempdir().unwrap();
    let drive = fixture(&root_dir);
    let root = drive.root().unwrap();
    let original = drive
        .mutate(
            "private-original-request",
            "private-actor",
            create(root.id, "private-name"),
        )
        .unwrap();
    let error = drive
        .mutate(
            "private-conflicting-request",
            "private-actor",
            create(root.id, "private-name"),
        )
        .unwrap_err();
    assert!(matches!(error, Error::Conflict));
    let wire = equal_browser_tool(Operation::CreateFile, error, Reason::Conflict);
    let text = std::str::from_utf8(&wire).unwrap();
    assert!(text.contains("file creation"));
    assert!(text.contains("current state"));
    assert!(!text.contains("private-"));
    assert!(!text.contains(root_dir.path().to_str().unwrap()));
    assert_eq!(
        drive.list(root.id, None, 10).unwrap().nodes,
        vec![original.node.clone()]
    );
    assert_eq!(
        drive.request_status("private-original-request").unwrap(),
        RequestStatus::Committed {
            result: original.clone()
        }
    );
    assert_eq!(
        drive.request_status("private-conflicting-request").unwrap(),
        RequestStatus::Uncommitted
    );
    drop(drive);
    let reopened = fixture(&root_dir);
    assert_eq!(reopened.metadata(original.node.id).unwrap(), original.node);
    assert_eq!(
        reopened.request_status("private-original-request").unwrap(),
        RequestStatus::Committed { result: original }
    );
    // No adapter decision or classification is consulted to read the receipt.
    assert_eq!(
        reopened
            .request_status("private-conflicting-request")
            .unwrap(),
        RequestStatus::Uncommitted
    );
}

#[test]
fn reused_request_identity_conflict_does_not_replace_the_committed_result() {
    let root_dir = tempfile::tempdir().unwrap();
    let drive = fixture(&root_dir);
    let root = drive.root().unwrap();
    let mutation = create(root.id, "original");
    let committed = drive
        .mutate("same-request", "actor", mutation.clone())
        .unwrap();
    let error = drive
        .mutate(
            "same-request",
            "actor",
            create(root.id, "different-private-payload"),
        )
        .unwrap_err();
    assert!(matches!(error, Error::Conflict));
    equal_browser_tool(Operation::CreateFile, error, Reason::Conflict);
    assert_eq!(
        drive.mutate("same-request", "actor", mutation).unwrap(),
        committed
    );
    assert_eq!(
        drive.request_status("same-request").unwrap(),
        RequestStatus::Committed {
            result: committed.clone()
        }
    );
    assert_eq!(
        drive.list(root.id, None, 10).unwrap().nodes,
        vec![committed.node]
    );
    assert_eq!(drive.blobs.list(None, 10).unwrap().len(), 1);
}

#[test]
fn stale_cas_and_pinned_read_keep_conflict_without_changing_current_content() {
    let root_dir = tempfile::tempdir().unwrap();
    let drive = fixture(&root_dir);
    let root = drive.root().unwrap();
    let initial = drive
        .mutate("initial", "actor", create(root.id, "file"))
        .unwrap()
        .node;
    let update = |bytes: &[u8]| Mutation::Update {
        id: initial.id,
        expected_mutation_id: initial.last_mutation_id.clone(),
        content_type: "text/plain".into(),
        bytes: bytes.to_vec(),
    };
    let current = drive
        .mutate("current", "actor", update(b"current-private-content"))
        .unwrap();
    let stale = update(b"stale-private-content");
    let kind = Operation::from(&stale);
    assert_eq!(kind, Operation::Update);
    let error = drive.mutate("stale", "actor", stale).unwrap_err();
    assert!(matches!(error, Error::Conflict));
    let wire = equal_browser_tool(kind, error, Reason::Conflict);
    assert!(std::str::from_utf8(&wire).unwrap().contains("file update"));
    let error = drive
        .read(initial.id, &initial.last_mutation_id, 0, 100)
        .unwrap_err();
    assert!(matches!(error, Error::Conflict));
    equal_browser_tool(Operation::Read, error, Reason::Conflict);
    assert_eq!(drive.metadata(initial.id).unwrap(), current.node);
    assert_eq!(
        drive
            .read(initial.id, &current.node.last_mutation_id, 0, 100)
            .unwrap()
            .bytes,
        b"current-private-content"
    );
    assert_eq!(
        drive.request_status("stale").unwrap(),
        RequestStatus::Uncommitted
    );
    assert_eq!(
        drive.request_status("current").unwrap(),
        RequestStatus::Committed { result: current }
    );
    assert_eq!(drive.blobs.list(None, 10).unwrap().len(), 2);
}

#[test]
fn current_grant_denial_does_not_publish_and_replay_still_checks_write_authority() {
    let root_dir = tempfile::tempdir().unwrap();
    let (raw, authority) = grant_fixture(&root_dir);
    let root = raw.root().unwrap();
    let bound = raw.with_authorizer(grant_authorizer);
    let mutation = create(root.id, "file");
    let committed = bound.mutate("original", "actor", mutation.clone()).unwrap();
    authority
        .execute("UPDATE grants SET write_access=0", [])
        .unwrap();
    assert!(bound.check_authorized(false).is_ok());
    let denied = bound
        .mutate("denied", "actor", create(root.id, "private-denied"))
        .unwrap_err();
    assert!(matches!(denied, Error::Denied));
    equal_browser_tool(Operation::CreateFile, denied, Reason::Denied);
    let denied_replay = bound.mutate("original", "actor", mutation).unwrap_err();
    assert!(matches!(denied_replay, Error::Denied));
    equal_browser_tool(Operation::CreateFile, denied_replay, Reason::Denied);
    assert_eq!(raw.metadata(committed.node.id).unwrap(), committed.node);
    assert_eq!(
        raw.request_status("denied").unwrap(),
        RequestStatus::Uncommitted
    );
    assert_eq!(
        raw.request_status("original").unwrap(),
        RequestStatus::Committed { result: committed }
    );
    assert_eq!(raw.blobs.list(None, 10).unwrap().len(), 1);
}

#[test]
fn deletion_fence_and_deleted_node_have_explicit_distinct_causes() {
    let root_dir = tempfile::tempdir().unwrap();
    let drive = fixture(&root_dir);
    let root = drive.root().unwrap();
    let file = drive
        .mutate("file", "actor", create(root.id, "file"))
        .unwrap()
        .node;
    let deletion = Mutation::Delete {
        id: file.id,
        expected_mutation_id: file.last_mutation_id.clone(),
    };
    assert_eq!(Operation::from(&deletion), Operation::Delete);
    drive.mutate("delete", "actor", deletion).unwrap();
    let error = drive.metadata(file.id).unwrap_err();
    assert!(matches!(error, Error::NotFound));
    equal_browser_tool(Operation::Metadata, error, Reason::NotFound);
    drive.fence().unwrap();
    let error = drive
        .mutate("fenced", "actor", create(root.id, "fenced"))
        .unwrap_err();
    assert!(matches!(error, Error::Fenced));
    equal_browser_tool(Operation::CreateFile, error, Reason::Fenced);
    assert_eq!(
        drive.request_status("fenced").unwrap(),
        RequestStatus::Uncommitted
    );
    assert!(matches!(
        drive.request_status("delete").unwrap(),
        RequestStatus::Committed { .. }
    ));
}

#[test]
fn real_metadata_failure_retains_sqlite_source_but_not_trigger_prose() {
    let root_dir = tempfile::tempdir().unwrap();
    let drive = fixture(&root_dir);
    let root = drive.root().unwrap();
    drive.database.transaction(|tx| {
        tx.execute_batch("CREATE TRIGGER fail_node BEFORE INSERT ON drive_nodes WHEN NEW.parent_id IS NOT NULL
            BEGIN SELECT RAISE(ABORT,'private-sql credential-secret /private/path'); END;")?;
        Ok(())
    }).unwrap();
    let error = drive
        .mutate("private-failed-request", "actor", create(root.id, "file"))
        .unwrap_err();
    assert!(matches!(error, Error::Sqlite(_)));
    let failure = DriveFailure::new(Operation::CreateFile, error);
    let Error::Sqlite(sqlite) = failure.source_error() else {
        panic!("original SQLite source");
    };
    assert!(sqlite.to_string().contains("credential-secret"));
    let wire = equal_browser_tool(
        Operation::CreateFile,
        failure.into_source(),
        Reason::MetadataFailure,
    );
    let text = std::str::from_utf8(&wire).unwrap();
    for secret in [
        "private-",
        "credential-secret",
        "/private/path",
        "CREATE TRIGGER",
        "fail_node",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
    assert!(text.contains("metadata database"));
    assert!(drive.list(root.id, None, 10).unwrap().nodes.is_empty());
    assert_eq!(
        drive.request_status("private-failed-request").unwrap(),
        RequestStatus::Uncommitted
    );
    // The same production ledger records no commit; projection adds no receipt.
    let receipts: i64 =
        drive
            .database
            .with_connection(|connection| {
                Ok(connection
                    .query_row("SELECT count(*) FROM drive_receipts", [], |row| row.get(0))?)
            })
            .unwrap();
    assert_eq!(receipts, 0);
}

#[test]
fn closed_source_mapping_and_all_audiences_exclude_private_diagnostics() {
    let encoding =
        serde_json::from_str::<Value>("credential-secret private-provider-body").unwrap_err();
    let cases = [
        (
            Error::Invalid("conflict safe-to-retry credential-secret /private/path".into()),
            Reason::Invalid,
        ),
        (Error::NotFound, Reason::NotFound),
        (Error::Conflict, Reason::Conflict),
        (Error::Denied, Reason::Denied),
        (Error::Fenced, Reason::Fenced),
        (
            Error::Storage(feature_storage::FeatureStorageError::Identity(
                "physicalidentity-secret requestID-secret credential-secret".into(),
            )),
            Reason::StorageFailure,
        ),
        (
            Error::Sqlite(rusqlite::Error::InvalidParameterName("private-sql".into())),
            Reason::MetadataFailure,
        ),
        (
            Error::Blob(blob::BlobError::Io {
                path: "/private/blob-path".into(),
                source: std::io::Error::other("credential-secret private-provider-body"),
            }),
            Reason::BlobFailure,
        ),
        (Error::Encoding(encoding), Reason::EncodingFailure),
    ];
    for (source, reason) in cases {
        let mut facts = FailureFacts::new(DriveFailure::new(Operation::Read, source)).unwrap();
        facts
            .with_context(
                OperationContext::new("arbitrarycontext-secret", "private-stage").unwrap(),
            )
            .unwrap();
        facts
            .set_correlation(CorrelationRef::new("requestID-secret").unwrap())
            .unwrap();
        let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
        let expected = encode_projected_failure(&browser, budget()).unwrap();
        assert_eq!(browser.details().reason, reason);
        macro_rules! check_audience {
            ($audience:ty) => {{
                assert!(matches!(
                    project_failure::<$audience, _>(&facts, &DriveFailureVisibility::default()),
                    Err(ContractError::ProjectionUnavailable)
                ));
                let projected = project_failure::<$audience, _>(&facts, &VISIBLE).unwrap();
                let bytes = encode_projected_failure(&projected, budget()).unwrap();
                assert_eq!(bytes.as_bytes(), expected.as_bytes());
            }};
        }
        check_audience!(Browser);
        check_audience!(Tool);
        check_audience!(InternalTransfer);
        check_audience!(Storage);
        check_audience!(Telemetry);
        let telemetry = project_failure::<Telemetry, _>(&facts, &VISIBLE).unwrap();
        let fields = observation_fields(&telemetry);
        assert_eq!(fields.reason, reason.as_str());
        assert_eq!(fields.operation, Some("read"));
        assert_eq!(fields.stage, Some("operation"));
        assert_eq!(fields.correlation, None);
        let text = std::str::from_utf8(expected.as_bytes()).unwrap();
        for secret in [
            "credential-secret",
            "private-",
            "/private/",
            "physicalidentity-secret",
            "requestID-secret",
            "arbitrarycontext-secret",
            "safe-to-retry",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[test]
fn stored_codec_facts_require_current_grant_and_rebuild_received_prose_and_context() {
    let root_dir = tempfile::tempdir().unwrap();
    let (raw, authority) = grant_fixture(&root_dir);
    let root = raw.root().unwrap();
    let bound = raw.with_authorizer(grant_authorizer);
    let original = bound
        .mutate("original", "actor", create(root.id, "file"))
        .unwrap();
    let error = bound
        .mutate("collision", "actor", create(root.id, "file"))
        .unwrap_err();
    let facts = FailureFacts::new(DriveFailure::new(Operation::CreateFile, error)).unwrap();
    let current_visibility = || DriveFailureVisibility {
        // In this fixture the existing read grant gates Drive cause visibility.
        // Production boundaries must also check their diagnostic visibility.
        can_view_cause: bound.check_authorized(false).is_ok(),
    };
    let saved = project_failure::<Storage, _>(&facts, &current_visibility()).unwrap();
    let saved = encode_projected_failure(&saved, budget()).unwrap();
    let mut wire: Value = serde_json::from_slice(saved.as_bytes()).unwrap();
    wire["explanation"] = json!("SUCCESS; resend automatically; receivedprose-secret");
    wire["context"]["correlation"] = json!("requestID-secret");
    wire["context"]["operations"][0]["operation"] = json!("arbitrarycontext-secret");
    wire["context"]["operations"][0]["operation_ref"] = json!("physicalidentity-secret");
    // A codec artifact, not a second operation ledger or a production writer.
    let saved_path = root_dir.path().join("failure-codec-artifact.json");
    std::fs::write(&saved_path, serde_json::to_vec(&wire).unwrap()).unwrap();
    let reread = || {
        let bytes = std::fs::read(&saved_path).unwrap();
        let DecodeFailure::Known(received) =
            decode_failure::<DriveFailureDetails>(&bytes, budget())
        else {
            panic!("supported codec artifact");
        };
        received
    };
    let received = reread();
    let browser = project_failure::<Browser, _>(received.facts(), &current_visibility()).unwrap();
    let tool = project_failure::<Tool, _>(received.facts(), &current_visibility()).unwrap();
    let clean = encode_projected_failure(&browser, budget()).unwrap();
    assert_eq!(
        clean.as_bytes(),
        encode_projected_failure(&tool, budget())
            .unwrap()
            .as_bytes()
    );
    assert_eq!(browser.details(), facts.cause().details());
    assert_eq!(clean.as_bytes(), saved.as_bytes());
    let text = std::str::from_utf8(clean.as_bytes()).unwrap();
    for secret in [
        "SUCCESS",
        "resend automatically",
        "receivedprose-secret",
        "requestID-secret",
        "arbitrarycontext-secret",
        "physicalidentity-secret",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
    authority
        .execute("UPDATE grants SET revoked=1", [])
        .unwrap();
    assert!(matches!(
        bound.request_status("original"),
        Err(Error::Denied)
    ));
    let received = reread();
    macro_rules! denied_after_revoke {
        ($audience:ty) => {
            assert!(matches!(
                project_failure::<$audience, _>(received.facts(), &current_visibility()),
                Err(ContractError::ProjectionUnavailable)
            ));
        };
    }
    denied_after_revoke!(Browser);
    denied_after_revoke!(Tool);
    denied_after_revoke!(InternalTransfer);
    denied_after_revoke!(Storage);
    denied_after_revoke!(Telemetry);
    assert_eq!(
        raw.request_status("original").unwrap(),
        RequestStatus::Committed { result: original }
    );
    assert_eq!(
        raw.request_status("collision").unwrap(),
        RequestStatus::Uncommitted
    );
    assert_eq!(raw.blobs.list(None, 10).unwrap().len(), 2);
}

#[test]
fn codec_rejects_mismatched_or_injected_details_without_guessing_future_causes() {
    let facts = FailureFacts::new(DriveFailure::new(Operation::Read, Error::Conflict)).unwrap();
    let projected = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    assert!(matches!(
        encode_projected_failure(&projected, Budget::new(20, 20)),
        Err(ContractError::TooLarge)
    ));
    let bytes = encode_projected_failure(&projected, budget()).unwrap();
    let original: Value = serde_json::from_slice(bytes.as_bytes()).unwrap();
    let mut future = original.clone();
    future["reason"] = json!("future_reason");
    assert!(matches!(
        decode_failure::<DriveFailureDetails>(&serde_json::to_vec(&future).unwrap(), budget()),
        DecodeFailure::Unsupported(_)
    ));
    let mut future_version = original.clone();
    future_version["details_version"] = json!(2);
    assert!(matches!(
        decode_failure::<DriveFailureDetails>(
            &serde_json::to_vec(&future_version).unwrap(),
            budget()
        ),
        DecodeFailure::Unsupported(_)
    ));
    let mut mismatch = original.clone();
    mismatch["details"]["reason"] = json!("denied");
    assert!(matches!(
        decode_failure::<DriveFailureDetails>(&serde_json::to_vec(&mismatch).unwrap(), budget()),
        DecodeFailure::Corrupt(ContractError::InvalidDetails)
    ));
    for (field, value) in [
        ("path", json!("/private/path")),
        ("request_id", json!("requestID-secret")),
        ("retryable", json!(true)),
        ("state", json!("unknown")),
    ] {
        let mut injected = original.clone();
        injected["details"][field] = value;
        assert!(matches!(
            decode_failure::<DriveFailureDetails>(
                &serde_json::to_vec(&injected).unwrap(),
                budget()
            ),
            DecodeFailure::Corrupt(ContractError::InvalidDetails)
        ));
    }
}
