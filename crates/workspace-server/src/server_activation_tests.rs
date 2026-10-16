// Production generated routes, SQLite and WebAuthn verifier share the passkey
// security fixture. Only the authenticator is SoftPasskey, not a browser/OS key.
// The ordinary Server test target selects these isolated integration cases.

impl Fixture {
    async fn activation_admin(&self) -> String {
        let (cookie, user) = self
            .enroll(
                "registration",
                None,
                "activation-admin",
                &mut authenticator(),
            )
            .await;
        self.api
            .config_store
            .setup_hosting_admin(&user.handle, false)
            .unwrap();
        let actor = RequestActor {
            user_id: user.user_id,
            account_id: user.account_id,
            handle: user.handle,
            display_name: user.display_name,
            auth_method: ActorAuthMethod::BrowserSession,
        };
        self.api
            .store
            .set_hosting_self_registration_enabled(&actor, false)
            .unwrap();
        cookie
    }

    async fn issue_activation(
        &self,
        admin: &str,
        handle: &str,
    ) -> server_api::ActivationCodeResponse {
        let (status, headers, body) = self
            .post(
                "/api/hosting/accounts",
                Some(admin),
                Some(ORIGIN_URL),
                serde_json::json!({"handle":handle}),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "code issuance must succeed without printing its response"
        );
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        serde_json::from_value(body).unwrap()
    }

    async fn activation_options(
        &self,
        issued: &server_api::ActivationCodeResponse,
    ) -> PasskeyRegistrationOptionsResponse {
        let (status, headers, body) = self
            .post(
                "/api/auth/activation/options",
                None,
                Some(ORIGIN_URL),
                serde_json::json!({"handle": issued.user.handle, "code": issued.code}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        assert!(body.get("access_token").is_none());
        serde_json::from_value(body).unwrap()
    }
}

#[tokio::test]
async fn activation_requires_verified_passkey_and_never_grants_hosting_authority() {
    let f = Fixture::new().await;
    let admin = f.activation_admin().await;
    let issued = f.issue_activation(&admin, "pending-person").await;
    assert!(!f.api.store.hosting_self_registration_enabled().unwrap());
    let abandoned = f.activation_options(&issued).await;
    assert!(
        f.api
            .store
            .is_user_pending_activation(&issued.user.user_id)
            .unwrap()
    );
    assert!(f.credentials(&issued.user).is_empty());
    rejected(
        f.post(
            "/api/auth/passkeys/login/options",
            None,
            None,
            serde_json::json!({"handle":issued.user.handle}),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "passkey",
    );
    // A valid attestation for a different challenge cannot activate this identity.
    let options = f.activation_options(&issued).await;
    let mut tampered = Fixture::credential(abandoned, &mut authenticator());
    tampered["challenge_id"] = Value::String(options.challenge_id);
    rejected(
        f.post("/api/auth/activation/complete", None, None, tampered)
            .await,
        StatusCode::BAD_REQUEST,
        "webauthn_registration_verification_failed",
    );
    assert!(
        f.api
            .store
            .is_user_pending_activation(&issued.user.user_id)
            .unwrap()
    );
    assert!(f.credentials(&issued.user).is_empty());
    // Cancellation/failure did not spend the activation code. Fresh options resume.
    let options = f.activation_options(&issued).await;
    let mut key = authenticator();
    let complete = Fixture::credential(options, &mut key);
    let (status, headers, body) = f
        .post(
            "/api/auth/activation/complete",
            None,
            None,
            complete.clone(),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(axum::http::header::SET_COOKIE));
    assert_eq!(body["user"]["user_id"], issued.user.user_id);
    assert!(
        !f.api
            .store
            .is_user_pending_activation(&issued.user.user_id)
            .unwrap()
    );
    assert!(!f.api.store.is_hosting_admin(&issued.user.user_id).unwrap());
    assert_eq!(f.credentials(&issued.user).len(), 1);
    rejected(
        f.post("/api/auth/activation/complete", None, None, complete)
            .await,
        StatusCode::BAD_REQUEST,
        "invalid_passkey_challenge",
    );
    let (status, headers, _) = f
        .post(
            "/api/auth/activation/options",
            None,
            None,
            serde_json::json!({"handle":issued.user.handle,"code":issued.code}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
    assert_eq!(f.login("pending-person", &mut key).await, issued.user);
}

#[tokio::test]
async fn activation_ceremonies_cannot_be_used_by_registration_or_addition_endpoints() {
    let f = Fixture::new().await;
    let admin = f.activation_admin().await;
    let issued = f.issue_activation(&admin, "purpose-bound").await;
    for endpoint in [
        "/api/auth/passkeys/registration/complete",
        "/api/auth/passkeys/addition/complete",
    ] {
        let options = f.activation_options(&issued).await;
        let complete = Fixture::credential(options, &mut authenticator());
        let (status, _, _) = f
            .post(endpoint, Some(&admin), Some(ORIGIN_URL), complete)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(f.credentials(&issued.user).is_empty());
        assert!(
            f.api
                .store
                .is_user_pending_activation(&issued.user.user_id)
                .unwrap()
        );
    }
    // The converse also fails: even a logged-in administrator's addition ceremony
    // must not become an activation or create credentials for the provisional user.
    let options = f.options("addition", Some(&admin), "ignored").await;
    let complete = Fixture::credential(options, &mut authenticator());
    rejected(
        f.post(
            "/api/auth/activation/complete",
            Some(&admin),
            Some(ORIGIN_URL),
            complete,
        )
        .await,
        StatusCode::BAD_REQUEST,
        "passkey_registration_binding_mismatch",
    );
    assert!(f.credentials(&issued.user).is_empty());
}

#[tokio::test]
async fn activation_reissue_invalidates_even_a_previously_verified_code_ceremony() {
    let f = Fixture::new().await;
    let admin = f.activation_admin().await;
    let issued = f.issue_activation(&admin, "rotated").await;
    let options = f.activation_options(&issued).await;
    let complete = Fixture::credential(options, &mut authenticator());
    let (status, headers, body) = f
        .post(
            "/api/hosting/accounts/activation-code",
            Some(&admin),
            Some(ORIGIN_URL),
            serde_json::json!({"handle":"rotated"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[CACHE_CONTROL], "no-store");
    let replacement: server_api::ActivationCodeResponse = serde_json::from_value(body).unwrap();
    assert!(
        issued.code != replacement.code,
        "reissue must replace the secret"
    );
    let (status, _, _) = f
        .post("/api/auth/activation/complete", None, None, complete)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(f.credentials(&issued.user).is_empty());
    assert!(
        f.api
            .store
            .is_user_pending_activation(&issued.user.user_id)
            .unwrap()
    );
    let _ = f.activation_options(&replacement).await;
}

#[tokio::test]
async fn activation_two_valid_ceremonies_complete_once_and_save_no_extra_credential() {
    let f = Fixture::new().await;
    let admin = f.activation_admin().await;
    let issued = f.issue_activation(&admin, "one-winner").await;
    let first = Fixture::credential(f.activation_options(&issued).await, &mut authenticator());
    let second = Fixture::credential(f.activation_options(&issued).await, &mut authenticator());
    let (a, b) = tokio::join!(
        f.post("/api/auth/activation/complete", None, None, first),
        f.post("/api/auth/activation/complete", None, None, second)
    );
    assert_eq!(
        [a.0, b.0]
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [a.0, b.0]
            .iter()
            .filter(|status| **status == StatusCode::BAD_REQUEST)
            .count(),
        1
    );
    assert_eq!(f.credentials(&issued.user).len(), 1);
}

#[tokio::test]
async fn activation_direct_admin_mutations_reject_foreign_origin_even_with_live_bearer() {
    use api_macros::HttpError;
    use server_api::ServerApi;
    let f = Fixture::new().await;
    let cookie = f.activation_admin().await;
    let token = f.device_token(&cookie).await;
    let service = ServerApiContractService::Workspace(f.api.clone());
    let context = |origin: Option<&str>| server_api::ServerRequestContext {
        actor: None,
        worker_source: None,
        runtime_source: None,
        // A forged extension cannot override the byte-exact transport Origin.
        origin: Some(ORIGIN_URL.into()),
        transport_headers: std::iter::once((
            "authorization".into(),
            format!("Bearer {token}").into_bytes(),
        ))
        .chain(origin.map(|value| ("origin".into(), value.as_bytes().to_vec())))
        .collect(),
    };
    for handle in ["new-id", "existing-pending"] {
        let request = || server_api::ProvisionalAccountRequest {
            handle: handle.into(),
        };
        assert_eq!(
            service
                .hosting_account_create(context(Some("https://foreign.test")), request())
                .await
                .unwrap_err()
                .status_code(),
            403
        );
        assert_eq!(
            service
                .hosting_activation_code_reissue(context(Some("https://foreign.test")), request())
                .await
                .unwrap_err()
                .status_code(),
            403
        );
        assert!(f.api.store.get_user_by_handle(handle).unwrap().is_none());
    }
    let issued = service
        .hosting_account_create(
            context(None),
            server_api::ProvisionalAccountRequest {
                handle: "non-browser-bearer".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(issued.user.handle, "non-browser-bearer");
}
