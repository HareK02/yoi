// The generated HTTP router, real SQLite and production verifier/session issuer
// are connected. Only the authenticator is the shared resident software model;
// SoftPasskey alone is not resident-capable and browser-native UX is not claimed.

impl Fixture {
    async fn discoverable_options(&self) -> PasskeyLoginOptionsResponse {
        let (status, _, body) = self
            .post(
                "/api/auth/passkeys/login/options",
                None,
                Some(ORIGIN_URL),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let options: PasskeyLoginOptionsResponse = serde_json::from_value(body).unwrap();
        assert!(options.public_key.public_key.allow_credentials.is_empty());
        assert!(options.public_key.mediation.is_none());
        assert_eq!(
            serde_json::to_value(&options.public_key).unwrap()["publicKey"]["userVerification"],
            "required"
        );
        let owner: Option<String> = self
            .api
            .config_store
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT user_id FROM auth_challenges WHERE challenge_id=?1",
                    [&options.challenge_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert!(
            owner.is_none(),
            "normal challenge must not preselect any account"
        );
        options
    }

    async fn discoverable_request(&self, key: &mut Authenticator) -> PasskeyLoginCompleteRequest {
        let options = self.discoverable_options().await;
        let credential = key
            .do_authentication(Url::parse(ORIGIN_URL).unwrap(), options.public_key)
            .unwrap();
        PasskeyLoginCompleteRequest {
            challenge_id: options.challenge_id,
            credential,
        }
    }

    async fn discoverable_login(&self, key: &mut Authenticator) -> AuthenticatedUser {
        let request = self.discoverable_request(key).await;
        let (status, headers, body) = self
            .post(
                "/api/auth/passkeys/login/complete",
                None,
                Some(ORIGIN_URL),
                serde_json::to_value(request).unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let user: AuthUserResponse = serde_json::from_value(body).unwrap();
        let cookie = headers[axum::http::header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let token = cookie.split_once('=').unwrap().1;
        let live = crate::auth::resolve_request_actor_sync(
            self.api.store.as_ref(),
            None,
            Some(token.to_string()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(live.user_id, user.user.user_id);
        assert_eq!(live.account_id, user.user.account_id);
        user.user
    }
}

#[tokio::test]
async fn resident_selection_with_multiple_accounts_and_all_enrollment_purposes_issues_only_owner_session()
 {
    let f = Fixture::new().await;
    let mut device = authenticator();
    let (alice_cookie, alice) = f.enroll("registration", None, "alice", &mut device).await;
    let alice_id = f.credentials(&alice)[0].credential_id.clone();
    let (_, bob) = f.enroll("registration", None, "bob", &mut device).await;
    let bob_id = f.credentials(&bob)[0].credential_id.clone();
    for (id, expected) in [(alice_id, alice.clone()), (bob_id, bob)] {
        let id: webauthn_rs::prelude::Base64UrlSafeData =
            serde_json::from_value(Value::String(id)).unwrap();
        device.select(&id);
        assert_eq!(f.discoverable_login(&mut device).await, expected);
    }
    let mut added = authenticator();
    f.enroll("addition", Some(&alice_cookie), "", &mut added)
        .await;
    assert_eq!(f.discoverable_login(&mut added).await, alice);
    // Registration policy controls account creation, not login or own addition.
    let admin = f.activation_admin().await;
    assert_eq!(f.discoverable_login(&mut added).await, alice);
    let issued = f.issue_activation(&admin, "pending-discoverable").await;
    let mut activated = authenticator();
    let options = f.activation_options(&issued).await;
    let body = Fixture::credential(options, &mut activated);
    let (status, _, body) = f
        .post(
            "/api/auth/activation/complete",
            None,
            Some(ORIGIN_URL),
            body,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.discoverable_login(&mut activated).await, issued.user);
}

#[tokio::test]
async fn discoverable_verifier_rejects_unknown_owner_signature_origin_rp_uv_expiry_reuse_and_revocation()
 {
    let f = Fixture::new().await;
    let mut key = authenticator();
    let (_, alice) = f.enroll("registration", None, "alice", &mut key).await;
    for defect in [
        "unknown",
        "handle",
        "signature",
        "origin",
        "rp",
        "uv",
        "expired",
        "consumed",
        "stored_id",
        "stored_owner",
    ] {
        let mut request = f.discoverable_request(&mut key).await;
        let original = f.credentials(&alice)[0].clone();
        let change_byte =
            |value: &mut webauthn_rs::prelude::Base64UrlSafeData, index: usize, bit: u8| {
                let mut bytes = value.to_vec();
                bytes[index] ^= bit;
                *value = bytes.into();
            };
        match defect {
            "unknown" => change_byte(&mut request.credential.raw_id, 0, 1),
            "handle" => request.credential.response.user_handle = Some(vec![0; 16].into()),
            "signature" => change_byte(&mut request.credential.response.signature, 0, 1),
            "origin" => {
                let mut data: Value =
                    serde_json::from_slice(&request.credential.response.client_data_json).unwrap();
                data["origin"] = Value::String("https://other.test".into());
                request.credential.response.client_data_json =
                    serde_json::to_vec(&data).unwrap().into();
            }
            "rp" => change_byte(&mut request.credential.response.authenticator_data, 0, 1),
            "uv" => change_byte(&mut request.credential.response.authenticator_data, 32, 4),
            "expired" => f.expire_challenge(&request.challenge_id),
            "consumed" => {
                f.api
                    .store
                    .consume_auth_challenge_by_id(
                        &request.challenge_id,
                        "passkey_login",
                        &crate::auth::now_rfc3339(),
                    )
                    .unwrap();
            }
            "stored_id" => {
                let mut corrupt = original.clone();
                let mut value: Value = serde_json::from_str(&corrupt.public_key_cose).unwrap();
                // Keep the indexed ID, replace the embedded verified credential ID.
                value["cred"]["cred_id"] = Value::String("YW5vdGhlci1jcmVkZW50aWFs".into());
                corrupt.public_key_cose = serde_json::to_string(&value).unwrap();
                f.api.store.upsert_passkey_credential(&corrupt).unwrap();
            }
            "stored_owner" => {
                let (_, bob) = f
                    .enroll("registration", None, "bob", &mut authenticator())
                    .await;
                f.api
                    .config_store
                    .with_conn(|conn| {
                        conn.execute(
                            "UPDATE passkey_credentials SET user_id=?1 WHERE credential_id=?2",
                            [&bob.user_id, &original.credential_id],
                        )?;
                        Ok(())
                    })
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let (status, headers, body) = f
            .post(
                "/api/auth/passkeys/login/complete",
                None,
                Some(ORIGIN_URL),
                serde_json::to_value(&request).unwrap(),
            )
            .await;
        assert!(status.is_client_error(), "{defect}: {body}");
        assert!(
            !headers.contains_key(axum::http::header::SET_COOKIE),
            "{defect}"
        );
        let (status, headers, _) = f
            .post(
                "/api/auth/passkeys/login/complete",
                None,
                Some(ORIGIN_URL),
                serde_json::to_value(request).unwrap(),
            )
            .await;
        assert!(
            status.is_client_error(),
            "failed attempts must consume their ceremony: {defect}"
        );
        assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        f.api
            .config_store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE passkey_credentials SET user_id=?1 WHERE credential_id=?2",
                    [&original.user_id, &original.credential_id],
                )?;
                Ok(())
            })
            .unwrap();
        f.api.store.upsert_passkey_credential(&original).unwrap();
    }
    assert_eq!(f.discoverable_login(&mut key).await, alice);
}

#[tokio::test]
async fn signed_assertions_with_foreign_origin_rp_or_without_uv_are_rejected_by_required_verifier()
{
    let f = Fixture::new().await;
    let mut key = authenticator();
    let (_, alice) = f.enroll("registration", None, "alice", &mut key).await;
    let id: webauthn_rs::prelude::Base64UrlSafeData = serde_json::from_value(Value::String(
        f.credentials(&alice)[0].credential_id.clone(),
    ))
    .unwrap();
    for defect in ["origin", "rp", "uv"] {
        let mut options = f.discoverable_options().await;
        let mut origin = ORIGIN_URL;
        match defect {
            "origin" => origin = "https://sub.workspace.test",
            "rp" => {
                origin = "https://other.test";
                options.public_key.public_key.rp_id = "other.test".into();
                // Explicit signer selection produces a genuinely signed assertion
                // for the wrong RP; normal Server options remain empty/unchanged.
                options.public_key.public_key.allow_credentials =
                    vec![webauthn_rs_proto::AllowCredentials {
                        type_: "public-key".into(),
                        id: id.clone(),
                        transports: None,
                    }];
            }
            "uv" => {
                options.public_key.public_key.user_verification =
                    webauthn_rs_proto::UserVerificationPolicy::Preferred
            }
            _ => unreachable!(),
        }
        let mut credential = key
            .do_authentication(Url::parse(origin).unwrap(), options.public_key)
            .unwrap();
        if defect == "uv" {
            assert_eq!(credential.response.authenticator_data[32] & 4, 0);
        }
        credential.response.user_handle = Some(
            passkey_user_handle(&alice.account_id)
                .as_bytes()
                .to_vec()
                .into(),
        );
        let request = PasskeyLoginCompleteRequest {
            challenge_id: options.challenge_id,
            credential,
        };
        rejected(
            f.post(
                "/api/auth/passkeys/login/complete",
                None,
                Some(ORIGIN_URL),
                serde_json::to_value(request).unwrap(),
            )
            .await,
            StatusCode::BAD_REQUEST,
            "webauthn_login_verification_failed",
        );
    }
    assert_eq!(f.discoverable_login(&mut key).await, alice);
}

#[tokio::test]
async fn non_resident_library_authenticator_refuses_registration_without_creating_identity() {
    use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
    let f = Fixture::new().await;
    let options = f.options("registration", None, "unsupported-key").await;
    let selection = options
        .public_key
        .public_key
        .authenticator_selection
        .as_ref()
        .unwrap();
    assert!(selection.require_resident_key);
    assert_eq!(
        selection.resident_key,
        Some(webauthn_rs_proto::ResidentKeyRequirement::Required)
    );
    assert_eq!(
        selection.user_verification,
        webauthn_rs_proto::UserVerificationPolicy::Required
    );
    let result = WebauthnAuthenticator::new(SoftPasskey::new(true))
        .do_registration(Url::parse(ORIGIN_URL).unwrap(), options.public_key);
    assert!(
        result.is_err(),
        "SoftPasskey itself must not be reported as resident-capable"
    );
    assert!(
        f.api
            .store
            .get_user_by_handle("unsupported-key")
            .unwrap()
            .is_none()
    );
}

const LOCAL_ORIGIN: &str = "http://localhost:8787";

impl Fixture {
    fn set_origins(&mut self, origins: &[&str]) {
        let AuthConfig::Passkey { public_urls, .. } = &mut self.api.config.auth;
        *public_urls = origins.iter().map(|origin| (*origin).to_owned()).collect();
        self.app =
            generated_auth_contract_router(ServerApiContractService::Workspace(self.api.clone()));
    }

    async fn login_options_at(
        &self,
        origin: &str,
        handle: Option<&str>,
    ) -> PasskeyLoginOptionsResponse {
        let (status, _, body) = self
            .post(
                "/api/auth/passkeys/login/options",
                None,
                Some(origin),
                serde_json::json!({"handle": handle}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_value(body).unwrap()
    }
}

#[tokio::test]
async fn multiple_origins_derive_distinct_rps_and_cookie_flags_for_registration_and_both_logins() {
    let mut f = Fixture::new().await;
    f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
    for (origin, rp, handle, secure) in [
        (ORIGIN_URL, "workspace.test", "remote", true),
        (LOCAL_ORIGIN, "localhost", "local", false),
    ] {
        let mut key = authenticator();
        let (status, _, options) = f
            .post(
                "/api/auth/passkeys/registration/options",
                None,
                Some(origin),
                serde_json::json!({"handle": handle}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{options}");
        let options: PasskeyRegistrationOptionsResponse = serde_json::from_value(options).unwrap();
        assert_eq!(options.public_key.public_key.rp.id, rp);
        let credential = key
            .do_registration(Url::parse(origin).unwrap(), options.public_key)
            .unwrap();
        let (status, headers, body) = f
            .post(
                "/api/auth/passkeys/registration/complete",
                None,
                Some(origin),
                serde_json::to_value(PasskeyRegistrationCompleteRequest {
                    challenge_id: options.challenge_id,
                    credential,
                })
                .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            headers[axum::http::header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("; Secure"),
            secure
        );
        let expected: AuthUserResponse = serde_json::from_value(body).unwrap();
        for handle in [None, Some(handle)] {
            let options = f.login_options_at(origin, handle).await;
            assert_eq!(options.public_key.public_key.rp_id, rp);
            let credential = key
                .do_authentication(Url::parse(origin).unwrap(), options.public_key)
                .unwrap();
            let (status, headers, body) = f
                .post(
                    "/api/auth/passkeys/login/complete",
                    None,
                    Some(origin),
                    serde_json::to_value(PasskeyLoginCompleteRequest {
                        challenge_id: options.challenge_id,
                        credential,
                    })
                    .unwrap(),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                serde_json::from_value::<AuthUserResponse>(body)
                    .unwrap()
                    .user,
                expected.user
            );
            let cookie = headers[axum::http::header::SET_COOKIE].to_str().unwrap();
            assert_eq!(cookie.contains("; Secure"), secure);
            for value in headers.get_all(axum::http::header::SET_COOKIE) {
                assert_eq!(value.to_str().unwrap().contains("; Secure"), secure);
            }
            let (status, clear, body) = f
                .post(
                    "/api/auth/logout",
                    Some(cookie.split(';').next().unwrap()),
                    Some(origin),
                    serde_json::json!({}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(
                clear[axum::http::header::SET_COOKIE]
                    .to_str()
                    .unwrap()
                    .contains("; Secure"),
                secure
            );
        }
    }
}

#[tokio::test]
async fn registration_and_login_completion_reject_removed_origin_or_different_allowed_transport_origin()
 {
    let mut f = Fixture::new().await;
    f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
    let mut key = authenticator();
    let (_, alice) = f.enroll("registration", None, "alice", &mut key).await;
    for remove in [false, true] {
        for handle in [None, Some("alice")] {
            f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
            let options = f.login_options_at(ORIGIN_URL, handle).await;
            let credential = key
                .do_authentication(Url::parse(ORIGIN_URL).unwrap(), options.public_key)
                .unwrap();
            if remove {
                f.set_origins(&[LOCAL_ORIGIN]);
            }
            let request = serde_json::to_value(PasskeyLoginCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            })
            .unwrap();
            rejected(
                f.post(
                    "/api/auth/passkeys/login/complete",
                    None,
                    Some(if remove { ORIGIN_URL } else { LOCAL_ORIGIN }),
                    request.clone(),
                )
                .await,
                StatusCode::BAD_REQUEST,
                if remove {
                    "webauthn_origin_not_allowed"
                } else {
                    "webauthn_origin_mismatch"
                },
            );
            f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
            rejected(
                f.post(
                    "/api/auth/passkeys/login/complete",
                    None,
                    Some(ORIGIN_URL),
                    request,
                )
                .await,
                StatusCode::BAD_REQUEST,
                "invalid_passkey_challenge",
            );
        }
        f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
        let handle = if remove { "removed" } else { "mismatched" };
        let options = f.options("registration", None, handle).await;
        let request = Fixture::credential(options, &mut authenticator());
        if remove {
            f.set_origins(&[LOCAL_ORIGIN]);
        }
        rejected(
            f.post(
                "/api/auth/passkeys/registration/complete",
                None,
                Some(if remove { ORIGIN_URL } else { LOCAL_ORIGIN }),
                request,
            )
            .await,
            StatusCode::BAD_REQUEST,
            if remove {
                "webauthn_origin_not_allowed"
            } else {
                "webauthn_origin_mismatch"
            },
        );
        assert!(f.api.store.get_user_by_handle(handle).unwrap().is_none());
    }
    assert_eq!(f.credentials(&alice).len(), 1);
}

#[tokio::test]
async fn multiple_allowed_origins_do_not_relax_signed_origin_or_rp_verification() {
    let mut f = Fixture::new().await;
    let alternate_port = "https://workspace.test:8443";
    f.set_origins(&[ORIGIN_URL, alternate_port, LOCAL_ORIGIN]);
    let mut key = authenticator();
    let (_, alice) = f.enroll("registration", None, "alice", &mut key).await;
    let id: webauthn_rs::prelude::Base64UrlSafeData = serde_json::from_value(Value::String(
        f.credentials(&alice)[0].credential_id.clone(),
    ))
    .unwrap();
    for handle in [None, Some("alice")] {
        for wrong_rp in [false, true] {
            let mut options = f.login_options_at(ORIGIN_URL, handle).await;
            let signed_origin = if wrong_rp {
                LOCAL_ORIGIN
            } else {
                alternate_port
            };
            if wrong_rp {
                options.public_key.public_key.rp_id = "localhost".into();
            }
            // Explicit software signer selection creates real signatures even for an
            // intentionally wrong RP. The server still verifies its saved ceremony.
            options.public_key.public_key.allow_credentials =
                vec![webauthn_rs_proto::AllowCredentials {
                    type_: "public-key".into(),
                    id: id.clone(),
                    transports: None,
                }];
            let mut credential = key
                .do_authentication(Url::parse(signed_origin).unwrap(), options.public_key)
                .unwrap();
            credential.response.user_handle = Some(
                passkey_user_handle(&alice.account_id)
                    .as_bytes()
                    .to_vec()
                    .into(),
            );
            rejected(
                f.post(
                    "/api/auth/passkeys/login/complete",
                    None,
                    Some(ORIGIN_URL),
                    serde_json::to_value(PasskeyLoginCompleteRequest {
                        challenge_id: options.challenge_id,
                        credential,
                    })
                    .unwrap(),
                )
                .await,
                StatusCode::BAD_REQUEST,
                "webauthn_login_verification_failed",
            );
        }
    }
}

#[tokio::test]
async fn origin_allowlist_rejects_host_only_matches_and_body_transport_disagreement() {
    let mut f = Fixture::new().await;
    f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
    for origin in [
        "http://workspace.test",
        "https://workspace.test:444",
        "http://localhost:5173",
        "https://sub.workspace.test",
        "https://workspace.test.evil.test",
        "null",
        "https://user@workspace.test",
        "https://workspace.test/path",
        "https://workspace.test?query",
        "https://workspace.test#fragment",
    ] {
        let (status, _, body) = f
            .post(
                "/api/auth/passkeys/login/options",
                None,
                Some(origin),
                serde_json::json!({"browser_origin": ORIGIN_URL}),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{origin}: {body}");
    }
    rejected(
        f.post(
            "/api/auth/passkeys/login/options",
            None,
            Some(LOCAL_ORIGIN),
            serde_json::json!({"browser_origin": ORIGIN_URL}),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "webauthn_origin_mismatch",
    );
    let options = f.login_options_at("https://WORKSPACE.TEST:443", None).await;
    assert_eq!(options.public_key.public_key.rp_id, "workspace.test");
    let (cookie, _) = f
        .enroll("registration", None, "csrf", &mut authenticator())
        .await;
    for origin in [None, Some("https://workspace.test:444")] {
        rejected(
            f.post(
                "/api/auth/passkeys/addition/options",
                Some(&cookie),
                origin,
                serde_json::json!({"browser_origin": LOCAL_ORIGIN}),
            )
            .await,
            StatusCode::FORBIDDEN,
            "origin",
        );
    }
    f.set_origins(&[]);
    rejected(
        f.post(
            "/api/auth/passkeys/login/options",
            None,
            None,
            serde_json::json!({}),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "webauthn_origin_not_allowed",
    );
}

#[tokio::test]
async fn cross_rp_addition_requires_current_owner_and_lists_only_safe_owner_metadata() {
    let mut f = Fixture::new().await;
    f.set_origins(&[ORIGIN_URL, LOCAL_ORIGIN]);
    let mut remote_key = authenticator();
    let (cookie, alice) = f
        .enroll("registration", None, "alice", &mut remote_key)
        .await;
    let (_, bob) = f
        .enroll("registration", None, "bob", &mut authenticator())
        .await;
    let token = f.device_token(&cookie).await;
    let (status, _, options) = f
        .post_authenticated(
            "/api/auth/passkeys/addition/options",
            None,
            Some(&token),
            Some(LOCAL_ORIGIN),
            serde_json::json!({"browser_origin": LOCAL_ORIGIN}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let options: PasskeyRegistrationOptionsResponse = serde_json::from_value(options).unwrap();
    assert_eq!(options.public_key.public_key.rp.id, "localhost");
    let mut local_key = authenticator();
    let credential = local_key
        .do_registration(Url::parse(LOCAL_ORIGIN).unwrap(), options.public_key)
        .unwrap();
    let (status, headers, body) = f
        .post_authenticated(
            "/api/auth/passkeys/addition/complete",
            None,
            Some(&token),
            Some(LOCAL_ORIGIN),
            serde_json::to_value(PasskeyRegistrationCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            })
            .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_value::<AuthUserResponse>(body)
            .unwrap()
            .user,
        alice
    );
    let local_cookie = headers[axum::http::header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(f.credentials(&alice).len(), 2);
    assert_eq!(f.credentials(&bob).len(), 1);
    for origin in [ORIGIN_URL, LOCAL_ORIGIN] {
        let response = f
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/config")
                    .header(ORIGIN, origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let config: AuthPublicConfig =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(config.origin, origin);
        assert_eq!(
            config.rp_id,
            Url::parse(origin).unwrap().host_str().unwrap()
        );
    }
    let (status, metadata) = f
        .get_authenticated("/api/auth/passkeys", Some(local_cookie), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{metadata}");
    assert_eq!(metadata["user"]["user_id"], alice.user_id);
    assert_eq!(metadata["keys"].as_array().unwrap().len(), 2);
    // Metadata is account-wide, not an RP inventory: no raw credential IDs or
    // public key material escape through the authenticated management listing.
    for key in f.credentials(&alice).into_iter().chain(f.credentials(&bob)) {
        assert!(!metadata.to_string().contains(&key.credential_id));
        assert!(!metadata.to_string().contains(&key.public_key_cose));
    }
    assert_eq!(f.discoverable_login(&mut remote_key).await, alice);
    let options = f.login_options_at(LOCAL_ORIGIN, None).await;
    let credential = local_key
        .do_authentication(Url::parse(LOCAL_ORIGIN).unwrap(), options.public_key)
        .unwrap();
    let (status, _, body) = f
        .post(
            "/api/auth/passkeys/login/complete",
            None,
            Some(LOCAL_ORIGIN),
            serde_json::to_value(PasskeyLoginCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            })
            .unwrap(),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_value::<AuthUserResponse>(body)
            .unwrap()
            .user,
        alice
    );
}

#[tokio::test]
async fn device_approval_preserves_unauthorized_status_and_rechecks_live_cookie_authority() {
    use server_api::ServerApi;
    let f = Fixture::new().await;
    let (cookie, _) = f
        .enroll("registration", None, "approver", &mut authenticator())
        .await;
    let service = ServerApiContractService::Workspace(f.api.clone());
    let api = ServerAuthApi::from(&f.api);
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::COOKIE, cookie.parse().unwrap());
    let actor = resolve_actor(&api, &headers).await.unwrap().unwrap();
    let approve = || DeviceLoginApproveRequest {
        user_code: "missing-code".into(),
    };
    // Historical actor and origin snapshots are not proof of current transport authentication.
    let forged = server_request_context(
        Some(actor.clone()),
        None,
        None,
        Some(ORIGIN_URL.into()),
        &HeaderMap::new(),
    );
    let error = service
        .auth_device_login_approve(forged, approve())
        .await
        .unwrap_err();
    assert_eq!(api_macros::HttpError::status_code(&error), 401);
    let (status, _, body) = f
        .post(
            "/api/auth/device-login/approve",
            None,
            Some(ORIGIN_URL),
            serde_json::json!({"user_code":"missing-code"}),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let missing_origin = server_request_context(
        Some(actor.clone()),
        None,
        None,
        Some(ORIGIN_URL.into()),
        &headers,
    );
    let error = service
        .auth_device_login_approve(missing_origin, approve())
        .await
        .unwrap_err();
    assert_eq!(api_macros::HttpError::status_code(&error), 403);
    headers.insert(ORIGIN, ORIGIN_URL.parse().unwrap());
    let revoked =
        server_request_context(Some(actor), None, None, Some(ORIGIN_URL.into()), &headers);
    let token = cookie.split_once('=').unwrap().1;
    f.api
        .store
        .revoke_browser_session(&token_hash(token), &crate::auth::now_rfc3339())
        .unwrap();
    let error = service
        .auth_device_login_approve(revoked, approve())
        .await
        .unwrap_err();
    assert_eq!(api_macros::HttpError::status_code(&error), 401);
}
