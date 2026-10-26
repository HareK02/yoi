// Integration boundary: generated HTTP admission -> real WebAuthn verifier ->
// SQLite credential/receipt transaction -> real session issuance. Only the
// authenticator is synthetic; failures are SQLite triggers, not success fakes.

impl Fixture {
    async fn addition_result(&self, cookie: &str, challenge_id: &str) -> (StatusCode, Value) {
        let mut request = self.expected_owner(Some(cookie), None).await;
        request["challenge_id"] = challenge_id.into();
        let (status, _, body) = self
            .post(
                "/api/auth/passkeys/addition/result",
                Some(cookie),
                Some(ORIGIN_URL),
                request,
            )
            .await;
        (status, body)
    }
}

#[tokio::test]
async fn addition_receipt_is_atomic_and_recovers_credential_commit_not_session_success() {
    for failure in [
        "before_credential",
        "receipt_insert",
        "session_insert",
        "lost_response",
        "lost_body_accepted_cookie",
    ] {
        let f = Fixture::new().await;
        let mut original = authenticator();
        let (cookie, owner) = f.enroll("registration", None, "owner", &mut original).await;
        let original_expectation = f.expected_owner(Some(&cookie), None).await;
        let options = f.options("addition", Some(&cookie), "").await;
        let challenge_id = options.challenge_id.clone();
        let mut added = authenticator();
        let request = Fixture::credential(options, &mut added);
        let (status, result) = f.addition_result(&cookie, &challenge_id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["status"], "unknown");
        let table = match failure {
            "before_credential" => Some("passkey_credentials"),
            "receipt_insert" => Some("passkey_addition_receipts"),
            "session_insert" => Some("browser_sessions"),
            _ => None,
        };
        if let Some(table) = table {
            f.api.config_store.with_conn(|conn| {
                conn.execute_batch(&format!("CREATE TRIGGER inject_passkey_failure BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;"))?;
                Ok(())
            }).unwrap();
        }
        // Discard the completion body; one variant still accepts Set-Cookie.
        let (status, headers, _) = f
            .post(
                "/api/auth/passkeys/addition/complete",
                Some(&cookie),
                Some(ORIGIN_URL),
                request.clone(),
            )
            .await;
        assert_eq!(
            status,
            if matches!(failure, "lost_response" | "lost_body_accepted_cookie") {
                StatusCode::OK
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        );
        if !matches!(failure, "lost_response" | "lost_body_accepted_cookie") {
            assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        }
        let saved = matches!(
            failure,
            "session_insert" | "lost_response" | "lost_body_accepted_cookie"
        );
        if failure == "lost_body_accepted_cookie" {
            let accepted_cookie = headers[axum::http::header::SET_COOKIE]
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap();
            let current = f.expected_owner(Some(accepted_cookie), None).await;
            assert!(
                current["expected_authentication_reference"]
                    != original_expectation["expected_authentication_reference"],
                "addition issued a distinct session"
            );
            let mut recovery = original_expectation.clone();
            recovery["challenge_id"] = challenge_id.clone().into();
            let (status, _, result) = f
                .post(
                    "/api/auth/passkeys/addition/result",
                    Some(accepted_cookie),
                    Some(ORIGIN_URL),
                    recovery,
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(result["status"], "saved");
            assert_eq!(result["user"]["user_id"], owner.user_id);
        }
        let (status, result) = f.addition_result(&cookie, &challenge_id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["status"], if saved { "saved" } else { "unknown" });
        assert_eq!(f.credentials(&owner).len(), if saved { 2 } else { 1 });
        let receipt_count = f
            .api
            .config_store
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM passkey_addition_receipts",
                    [],
                    |row| row.get::<_, u64>(0),
                )?)
            })
            .unwrap();
        assert_eq!(receipt_count, u64::from(saved));
        if saved {
            assert_eq!(result["user"]["user_id"], owner.user_id);
            let (_, listed) = f
                .get_authenticated("/api/auth/passkeys", Some(&cookie), None)
                .await;
            let reference = result["key"]["key_reference"].as_str().unwrap();
            assert!(
                listed["keys"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|key| key["key_reference"] == reference)
            );
            assert!(result["key"]["normal_login_verified_at"].is_null());
        }
        if table.is_some() {
            f.api
                .config_store
                .with_conn(|conn| {
                    conn.execute_batch("DROP TRIGGER inject_passkey_failure")?;
                    Ok(())
                })
                .unwrap();
        }
        // Consumed completion is not retry authority, even after a transaction failure.
        let (status, headers, _) = f
            .post(
                "/api/auth/passkeys/addition/complete",
                Some(&cookie),
                Some(ORIGIN_URL),
                request,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        assert_eq!(f.credentials(&owner).len(), if saved { 2 } else { 1 });
        if saved {
            assert_eq!(f.discoverable_login(&mut added).await, owner);
        }
    }
}

#[tokio::test]
async fn stale_displayed_owner_or_session_cannot_start_addition_or_read_another_binding_result() {
    let f = Fixture::new().await;
    let mut alice_key = authenticator();
    let (alice_cookie, alice) = f
        .enroll("registration", None, "alice", &mut alice_key)
        .await;
    let (bob_cookie, _) = f
        .enroll("registration", None, "bob", &mut authenticator())
        .await;
    let expected = f.expected_owner(Some(&alice_cookie), None).await;
    let (replacement_cookie, _) = f.login_session("alice", &mut alice_key).await;
    let count_challenges = || {
        f.api
            .config_store
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT COUNT(*) FROM auth_challenges", [], |row| {
                        row.get::<_, u64>(0)
                    })?,
                )
            })
            .unwrap()
    };
    let before = count_challenges();
    for cookie in [&bob_cookie, &replacement_cookie] {
        let (status, _, _) = f
            .post(
                "/api/auth/passkeys/addition/options",
                Some(cookie),
                Some(ORIGIN_URL),
                expected.clone(),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            count_challenges(),
            before,
            "stale options must not persist a ceremony"
        );
    }
    let options = f.options("addition", Some(&alice_cookie), "").await;
    let challenge_id = options.challenge_id.clone();
    let request = Fixture::credential(options, &mut authenticator());
    assert_eq!(
        f.post(
            "/api/auth/passkeys/addition/complete",
            Some(&alice_cookie),
            Some(ORIGIN_URL),
            request
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.addition_result(&alice_cookie, &challenge_id).await.1["status"],
        "saved"
    );
    // Current owner authorization is separate from original operation binding.
    let mut recovery = expected.clone();
    recovery["challenge_id"] = challenge_id.clone().into();
    let (status, _, result) = f
        .post(
            "/api/auth/passkeys/addition/result",
            Some(&replacement_cookie),
            Some(ORIGIN_URL),
            recovery.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["status"], "saved");
    assert_eq!(
        f.post(
            "/api/auth/passkeys/addition/result",
            Some(&bob_cookie),
            Some(ORIGIN_URL),
            recovery.clone()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.post(
            "/api/auth/passkeys/addition/result",
            None,
            Some(ORIGIN_URL),
            recovery.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    recovery["expected_authentication_reference"] = "changed-original-reference".into();
    assert_eq!(
        f.post(
            "/api/auth/passkeys/addition/result",
            Some(&replacement_cookie),
            Some(ORIGIN_URL),
            recovery
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // Bob's own expected fields cannot discover Alice's operation either.
    let (status, result) = f.addition_result(&bob_cookie, &challenge_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result, serde_json::json!({"status":"unknown"}));
    assert_eq!(f.credentials(&alice).len(), 2);
    assert_eq!(
        f.post(
            "/api/auth/logout",
            Some(&alice_cookie),
            Some(ORIGIN_URL),
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let mut request = expected;
    request["challenge_id"] = challenge_id.into();
    assert_eq!(
        f.post(
            "/api/auth/passkeys/addition/result",
            Some(&alice_cookie),
            Some(ORIGIN_URL),
            request
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn own_key_list_is_safe_and_only_verified_normal_login_marks_confirmation() {
    let f = Fixture::new().await;
    let mut key = authenticator();
    let (cookie, owner) = f.enroll("registration", None, "owner", &mut key).await;
    let (other_cookie, _) = f
        .enroll("registration", None, "other", &mut authenticator())
        .await;
    assert_eq!(
        f.get_authenticated("/api/auth/passkeys", None, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, list) = f
        .get_authenticated("/api/auth/passkeys", Some(&cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["user"]["user_id"], owner.user_id);
    assert_eq!(list["keys"].as_array().unwrap().len(), 1);
    let summary = &list["keys"][0];
    assert!(summary["normal_login_verified_at"].is_null());
    assert!(summary["last_used_at"].is_null());
    let reference = summary["key_reference"].as_str().unwrap();
    assert_eq!(reference.len(), 32);
    assert_ne!(reference, f.credentials(&owner)[0].credential_id);
    assert_eq!(
        summary
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "key_reference",
            "created_at",
            "last_used_at",
            "normal_login_verified_at"
        ]
        .into_iter()
        .collect()
    );
    let (_, other) = f
        .get_authenticated("/api/auth/passkeys", Some(&other_cookie), None)
        .await;
    assert_ne!(other["keys"][0]["key_reference"], reference);
    assert_eq!(f.login("owner", &mut key).await, owner);
    let (_, legacy) = f
        .get_authenticated("/api/auth/passkeys", Some(&cookie), None)
        .await;
    assert!(legacy["keys"][0]["last_used_at"].is_string());
    assert!(legacy["keys"][0]["normal_login_verified_at"].is_null());
    // Rejected normal assertion does not produce confirmation metadata.
    let mut invalid = f.discoverable_request(&mut key).await;
    invalid.credential.response.signature = vec![0u8; 64].into();
    assert_eq!(
        f.post(
            "/api/auth/passkeys/login/complete",
            None,
            Some(ORIGIN_URL),
            serde_json::to_value(invalid).unwrap()
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(
        f.api.store.list_passkey_summaries(&owner.user_id).unwrap()[0]
            .normal_login_verified_at
            .is_none()
    );
    assert_eq!(f.discoverable_login(&mut key).await, owner);
    let (_, confirmed) = f
        .get_authenticated("/api/auth/passkeys", Some(&cookie), None)
        .await;
    assert_eq!(confirmed["keys"][0]["key_reference"], reference);
    assert!(confirmed["keys"][0]["normal_login_verified_at"].is_string());
}

#[tokio::test]
async fn unknown_transport_metadata_does_not_discard_verified_registration() {
    let f = Fixture::new().await;
    let options = f.options("registration", None, "future-transport").await;
    let mut key = authenticator();
    let mut request = Fixture::credential(options, &mut key);
    request["credential"]["response"]["transports"] =
        serde_json::json!(["internal", "future-transport", "test"]);
    let (status, _, body) = f
        .post(
            "/api/auth/passkeys/registration/complete",
            None,
            Some(ORIGIN_URL),
            request,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let user = serde_json::from_value::<AuthUserResponse>(body)
        .unwrap()
        .user;
    assert_eq!(f.discoverable_login(&mut key).await, user);
}
