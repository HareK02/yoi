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
