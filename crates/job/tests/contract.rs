use job::*;
use serde_json::json;

fn request() -> JobRequest {
    JobRequest {
        job_id: "check:r7".into(),
        purpose: "snapshot_check".into(),
        input_revision: "7".into(),
        input_ref: "snapshot://7".into(),
        input: json!({"title": "Check me"}),
        instruction: "Check this immutable snapshot.".into(),
        profile: "project:custom-check".into(),
        serialization_key: None,
        limits: JobLimits::default(),
    }
}

#[test]
fn shared_lifecycle_enums_round_trip_all_host_states() {
    for (state, name) in [
        (JobState::Pending, "pending"),
        (JobState::Completed, "completed"),
        (JobState::Failed, "failed"),
        (JobState::Unknown, "unknown"),
        (JobState::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<JobState>(json!(name)).unwrap(),
            state
        );
    }
    for (state, name) in [
        (JobAttemptState::Reserved, "reserved"),
        (JobAttemptState::Dispatching, "dispatching"),
        (JobAttemptState::Dispatched, "dispatched"),
        (JobAttemptState::Completed, "completed"),
        (JobAttemptState::Failed, "failed"),
        (JobAttemptState::Unknown, "unknown"),
        (JobAttemptState::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<JobAttemptState>(json!(name)).unwrap(),
            state
        );
    }
    assert!(serde_json::from_value::<JobState>(json!("running")).is_err());
    assert!(serde_json::from_value::<JobAttemptState>(json!("idle")).is_err());
}

#[test]
fn outcome_is_a_neutral_read_snapshot_with_optional_result_and_failure() {
    let mut outcome = JobOutcome {
        job_id: "job-1".into(),
        input_revision: "7".into(),
        attempt_id: "job-1:attempt:2".into(),
        attempt: 2,
        state: JobState::Pending,
        attempt_state: JobAttemptState::Dispatching,
        result: None,
        failure_category: None,
        failure_detail: None,
    };
    let encoded = serde_json::to_value(&outcome).unwrap();
    assert_eq!(
        encoded,
        json!({
            "job_id": "job-1", "input_revision": "7", "attempt_id": "job-1:attempt:2",
            "attempt": 2, "state": "pending", "attempt_state": "dispatching",
            "result": null, "failure_category": null, "failure_detail": null
        })
    );
    assert_eq!(
        serde_json::from_value::<JobOutcome>(encoded.clone()).unwrap(),
        outcome
    );
    let mut with_host_identity = encoded;
    with_host_identity["workspace_id"] = json!("workspace-a");
    assert!(serde_json::from_value::<JobOutcome>(with_host_identity).is_err());
    for (state, attempt_state, result, category, detail) in [
        (
            JobState::Completed,
            JobAttemptState::Completed,
            Some(json!({"ok": true})),
            None,
            None,
        ),
        (
            JobState::Failed,
            JobAttemptState::Failed,
            None,
            Some("execution"),
            Some("known failure"),
        ),
        (
            JobState::Unknown,
            JobAttemptState::Unknown,
            None,
            Some("interrupted"),
            Some("requires reconciliation"),
        ),
        (
            JobState::Cancelled,
            JobAttemptState::Cancelled,
            None,
            Some("cancelled"),
            Some("caller cancelled"),
        ),
    ] {
        outcome.state = state;
        outcome.attempt_state = attempt_state;
        outcome.result = result;
        outcome.failure_category = category.map(str::to_owned);
        outcome.failure_detail = detail.map(str::to_owned);
        assert_eq!(
            serde_json::from_str::<JobOutcome>(&serde_json::to_string(&outcome).unwrap()).unwrap(),
            outcome
        );
    }
}

#[test]
fn neutral_request_and_result_have_strict_round_trip_contracts() {
    let request = request();
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(encoded.as_object().unwrap().len(), 8);
    assert!(encoded.get("serialization_key").is_none());
    assert_eq!(
        serde_json::from_value::<JobRequest>(encoded.clone()).unwrap(),
        request
    );
    let mut with_grant = encoded.clone();
    with_grant["grants"] = json!({});
    assert!(serde_json::from_value::<JobRequest>(with_grant).is_err());
    let mut with_provenance = encoded;
    with_provenance["source_worker"] = json!({});
    assert!(serde_json::from_value::<JobRequest>(with_provenance).is_err());

    let submission = JobResultSubmission {
        job_id: request.job_id,
        attempt_id: "check:r7:attempt:1".into(),
        input_revision: request.input_revision,
        result: json!({"ok": true}),
    };
    let mut encoded = serde_json::to_value(&submission).unwrap();
    assert_eq!(
        serde_json::from_value::<JobResultSubmission>(encoded.clone()).unwrap(),
        submission
    );
    encoded["authority"] = json!("profile");
    assert!(serde_json::from_value::<JobResultSubmission>(encoded).is_err());
    assert_eq!(
        serde_json::from_str::<JobLimits>("{}").unwrap(),
        JobLimits::default()
    );
    assert!(serde_json::from_str::<JobLimits>(r#"{"turns":999}"#).is_err());
}

#[test]
fn fingerprint_covers_every_immutable_field_and_preserves_replay() {
    let request = request();
    let digest = request.fingerprint().unwrap();
    let decoded: JobRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(digest, decoded.fingerprint().unwrap());
    assert_eq!(digest, fingerprint(&request).unwrap());
    for mutate in [
        |r: &mut JobRequest| r.job_id.push('x'),
        |r: &mut JobRequest| r.purpose.push('x'),
        |r: &mut JobRequest| r.input_revision.push('x'),
        |r: &mut JobRequest| r.input_ref.push('x'),
        |r: &mut JobRequest| r.input = json!({"title": "changed"}),
        |r: &mut JobRequest| r.instruction.push('x'),
        |r: &mut JobRequest| r.profile = "builtin:backend-job".into(),
        |r: &mut JobRequest| r.serialization_key = Some("resource:7".into()),
        |r: &mut JobRequest| r.limits.max_attempts = 3,
    ] {
        let mut changed = request.clone();
        mutate(&mut changed);
        assert_ne!(digest, changed.fingerprint().unwrap());
    }
}

#[test]
fn registry_profiles_are_not_authority_or_a_fixed_allowlist() {
    for profile in [
        "default",
        "custom",
        "project:custom",
        "user:custom",
        "builtin:future-job",
    ] {
        profile_selector(profile).unwrap();
    }
    for profile in [
        "",
        " inherit",
        "inherit",
        "path:custom",
        "./a.toml",
        "/tmp/a",
        "project:../a",
        "project:a.toml",
        "builtin:",
        "unknown:a",
        "project:a:b",
        "C:\\custom",
        "{\"worker\":{}}",
    ] {
        assert!(profile_selector(profile).is_err(), "accepted {profile}");
    }
}

#[test]
fn validation_enforces_utf8_bytes_and_metadata_bounds() {
    let mut r = request();
    r.job_id = "x".repeat(256);
    r.input_revision = "x".repeat(256);
    r.purpose = "x".repeat(MAX_JOB_PURPOSE_BYTES);
    r.input_ref = "x".repeat(MAX_JOB_REFERENCE_BYTES);
    r.instruction = "x".repeat(MAX_JOB_INSTRUCTION_BYTES);
    r.serialization_key = Some("x".repeat(MAX_JOB_REFERENCE_BYTES));
    r.input = json!("x".repeat(MAX_JOB_INPUT_BYTES - 2));
    r.validate().unwrap();
    for mutate in [
        |r: &mut JobRequest| r.job_id.push('x'),
        |r: &mut JobRequest| r.input_revision.push('x'),
        |r: &mut JobRequest| r.purpose.push('x'),
        |r: &mut JobRequest| r.input_ref.push('x'),
        |r: &mut JobRequest| r.instruction.push('x'),
        |r: &mut JobRequest| r.serialization_key.as_mut().unwrap().push('x'),
        |r: &mut JobRequest| r.input = json!("x".repeat(MAX_JOB_INPUT_BYTES - 1)),
    ] {
        let mut changed = r.clone();
        mutate(&mut changed);
        assert!(changed.validate().is_err());
        assert!(changed.fingerprint().is_err());
    }
    assert!(validate_identifier("id", "a\nb", 256).is_err());
    assert!(validate_bounded_text("text", " untrimmed", 256).is_err());
    assert!(validate_bounded_text("text", "", 256).is_err());
    assert!(validate_bounded_text("text", "界", 2).is_err());
    validate_bounded_text("text", "a\nb", 256).unwrap();
}

#[test]
fn all_limit_dimensions_reject_zero_and_above_absolute_bounds() {
    let maximum = JobLimits {
        max_concurrent_jobs: ABSOLUTE_MAX_CONCURRENT_JOBS,
        timeout_seconds: MAX_JOB_TIMEOUT_SECONDS,
        max_result_bytes: ABSOLUTE_MAX_RESULT_BYTES,
        max_attempts: ABSOLUTE_MAX_ATTEMPTS,
    };
    maximum.validate().unwrap();
    for mutate in [
        |l: &mut JobLimits| l.max_concurrent_jobs = 0,
        |l: &mut JobLimits| l.max_concurrent_jobs += 1,
        |l: &mut JobLimits| l.timeout_seconds = 0,
        |l: &mut JobLimits| l.timeout_seconds += 1,
        |l: &mut JobLimits| l.max_result_bytes = 0,
        |l: &mut JobLimits| l.max_result_bytes += 1,
        |l: &mut JobLimits| l.max_attempts = 0,
        |l: &mut JobLimits| l.max_attempts += 1,
    ] {
        let mut changed = maximum.clone();
        mutate(&mut changed);
        assert!(changed.validate().is_err());
    }
}

#[test]
fn result_digest_and_failure_detail_bound_exact_utf8_encoding() {
    let result = json!({"ok": true});
    let (digest, encoded) = result_digest(&result, 11).unwrap();
    assert_eq!(encoded, r#"{"ok":true}"#);
    assert_eq!(digest, sha256(encoded.as_bytes()));
    assert!(result_digest(&result, 10).is_err());
    let unicode = json!("界");
    assert!(result_digest(&unicode, 4).is_err());
    result_digest(&unicode, 5).unwrap();
    let detail = "界".repeat(MAX_JOB_FAILURE_BYTES);
    let bounded = bounded_failure_detail(&detail);
    assert_eq!(bounded.len(), MAX_JOB_FAILURE_BYTES - 1);
    assert!(detail.starts_with(&bounded));
    assert_eq!(bounded_failure_detail("known failure"), "known failure");
}

#[test]
fn worker_input_is_revision_and_attempt_bound_not_final_prose_authority() {
    let request = request();
    let attempt = attempt_id(&request.job_id, 2);
    assert_eq!(attempt, "check:r7:attempt:2");
    let input = request.worker_input(&attempt).unwrap();
    assert!(input.starts_with(&request.instruction));
    for field in [
        "Job envelope (immutable):",
        "job_id: check:r7",
        "attempt_id: check:r7:attempt:2",
        "input_revision: 7",
        "input_ref: snapshot://7",
        r#"input_json: {"title":"Check me"}"#,
        "structured Job result capability",
        "Final prose and Worker Idle/Stopped state are not result authority.",
    ] {
        assert!(input.contains(field), "missing {field}");
    }
}
