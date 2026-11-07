use error_contract::{compatibility::*, problem::*, test_support::*, *};

fn main() -> Result<(), ContractError> {
    let access = CurrentAccess {
        can_view_resource: true,
        language: Language::English,
    };
    let budget = Budget::new(1024 * 1024, 16 * 1024).with_depth(64, 15);
    let subject = subject_facts();
    let browser = project_failure::<Browser, _>(&subject, &access)?;
    let response =
        to_problem_details(&browser, &WorkerApi, "urn:yoi:occurrence:example-1", budget)?;
    assert_eq!(response.status, 409); // The inner Subject API would classify this as 403.
    assert_no_fixture_secrets(response.body());

    let workdir = workdir_facts();
    let tool = project_failure::<Tool, _>(&workdir, &access)?;
    let encoded = encode_projected_failure(&tool, budget)?;
    assert_semantic_roundtrip(encoded.as_bytes(), tool.details(), budget);

    // Ship this reader before switching any existing result writer. Old diagnostic prose is
    // deliberately not an input to classification. The real domain result remains separate.
    assert!(matches!(
        read_failure_attachment::<SubjectDenied>(None, budget),
        StoredFailureRead::LegacyWithoutFacts
    ));
    let storage = project_failure::<Storage, _>(&subject, &access)?;
    let stored = encode_projected_failure(&storage, budget)?;
    let received = match read_failure_attachment::<SubjectDenied>(Some(stored.as_bytes()), budget) {
        StoredFailureRead::Structured(DecodeFailure::Known(value)) => value,
        _ => return Err(ContractError::Invalid),
    };
    let current_access = CurrentAccess {
        can_view_resource: false,
        language: Language::Japanese,
    };
    let reread = project_failure::<Browser, _>(received.facts(), &current_access)?;
    assert_eq!(
        reread.details().required_operation,
        RequiredOperation::SessionRecord
    );
    assert!(reread.details().subject_ref.is_none());

    let telemetry = project_failure::<Telemetry, _>(&subject, &access)?;
    let fields = observation_fields(&telemetry);
    assert_eq!(fields.stage, Some("register"));
    // Feed fields to the existing boundary's tracing sink; never log the source chain here.
    Ok(())
}
