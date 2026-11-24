// Component boundary: generated HTTP -> real WebAuthn -> real isolated SQLite.
// Only the authenticator is synthetic; rollback uses SQLite failure triggers.
mod addition_code {
    use super::*;
    const TARGET: &str = "https://second.test";
    const BASE: &str = "/api/auth/passkeys/addition-code";

    async fn fixture() -> Fixture {
        let mut f = Fixture::new().await;
        let AuthConfig::Passkey { public_urls, .. } = &mut f.api.config.auth;
        public_urls.push(TARGET.into());
        f.app = generated_auth_contract_router(ServerApiContractService::Workspace(f.api.clone()));
        f
    }

    async fn issue(f: &Fixture, cookie: &str) -> Value {
        let mut request = f.expected_owner(Some(cookie), None).await;
        request["target_origin"] = TARGET.into();
        let (status, headers, body) = f.post(BASE, Some(cookie), Some(ORIGIN_URL), request).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(headers[CACHE_CONTROL], "no-store");
        assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        assert_eq!(body["code"].as_str().unwrap().len(), 64);
        body
    }

    async fn options(
        f: &Fixture,
        issued: &Value,
        cookie: Option<&str>,
        origin: &str,
    ) -> (StatusCode, HeaderMap, Value) {
        f.post(
            &format!("{BASE}/options"),
            cookie,
            Some(origin),
            serde_json::json!({
                "handle": issued["user"]["handle"], "code": issued["code"], "browser_origin":origin,
            }),
        )
        .await
    }

    fn credential(body: Value, key: &mut Authenticator) -> Value {
        let options: PasskeyRegistrationOptionsResponse = serde_json::from_value(body).unwrap();
        let credential = key
            .do_registration(Url::parse(TARGET).unwrap(), options.public_key)
            .unwrap();
        serde_json::to_value(PasskeyRegistrationCompleteRequest {
            challenge_id: options.challenge_id,
            credential,
        })
        .unwrap()
    }

    async fn complete(
        f: &Fixture,
        request: Value,
        cookie: Option<&str>,
    ) -> (StatusCode, HeaderMap, Value) {
        f.post(&format!("{BASE}/complete"), cookie, Some(TARGET), request)
            .await
    }

    async fn active(f: &Fixture, cookie: &str) -> Value {
        let (status, body) = f.get_authenticated(BASE, Some(cookie), None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.get("code").is_none());
        assert!(body["active"].get("code").is_none());
        body["active"].clone()
    }

    #[tokio::test]
    async fn cross_domain_addition_preserves_identity_old_key_session_and_supports_login() {
        let f = fixture().await;
        let mut old_key = authenticator();
        let (cookie, owner) = f.enroll("registration", None, "owner", &mut old_key).await;
        let before = f.credentials(&owner);
        let issued = issue(&f, &cookie).await;
        assert_eq!(
            active(&f, &cookie).await["issuance_id"],
            issued["issuance_id"]
        );
        let (status, headers, body) = options(&f, &issued, None, TARGET).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
        let mut new_key = authenticator();
        let request = credential(body, &mut new_key);
        let (status, headers, body) = complete(&f, request.clone(), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["user"], serde_json::to_value(&owner).unwrap());
        assert!(headers.contains_key(axum::http::header::SET_COOKIE));
        assert_eq!(f.credentials(&owner).len(), 2);
        assert!(f.credentials(&owner).contains(&before[0]));
        assert!(active(&f, &cookie).await.is_null());
        assert_eq!(complete(&f, request, None).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            options(&f, &issued, None, TARGET).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(f.login_session("owner", &mut old_key).await.1, owner);
        let (status, _, body) = f
            .post(
                "/api/auth/passkeys/login/options",
                None,
                Some(TARGET),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let options: PasskeyLoginOptionsResponse = serde_json::from_value(body).unwrap();
        let assertion = new_key
            .do_authentication(Url::parse(TARGET).unwrap(), options.public_key)
            .unwrap();
        let (status, _, body) = f
            .post(
                "/api/auth/passkeys/login/complete",
                None,
                Some(TARGET),
                serde_json::to_value(PasskeyLoginCompleteRequest {
                    challenge_id: options.challenge_id,
                    credential: assertion,
                })
                .unwrap(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["user"], serde_json::to_value(owner).unwrap());
    }

    #[tokio::test]
    async fn issuance_requires_live_recent_browser_owner_fences_and_actual_origin() {
        let f = fixture().await;
        let (cookie, _) = f
            .enroll("registration", None, "owner", &mut authenticator())
            .await;
        let mut request = f.expected_owner(Some(&cookie), None).await;
        request["target_origin"] = TARGET.into();
        assert_eq!(
            f.post(BASE, Some(&cookie), None, request.clone()).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.post(
                BASE,
                Some(&cookie),
                Some("https://evil.test"),
                request.clone()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let mut changed = request.clone();
        changed["expected_account_id"] = "other".into();
        assert_eq!(
            f.post(BASE, Some(&cookie), Some(ORIGIN_URL), changed)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let mut changed = request.clone();
        changed["target_origin"] = "https://evil.test".into();
        assert_eq!(
            f.post(BASE, Some(&cookie), Some(ORIGIN_URL), changed)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let bearer = f.device_token(&cookie).await;
        let mut token_request = f.expected_owner(None, Some(&bearer)).await;
        token_request["target_origin"] = TARGET.into();
        assert_ne!(
            f.post_authenticated(BASE, None, Some(&bearer), Some(ORIGIN_URL), token_request)
                .await
                .0,
            StatusCode::OK
        );
        let issued = issue(&f, &cookie).await;
        f.api
            .config_store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE browser_sessions SET created_at=?1",
                    [rfc3339_after(Duration::minutes(-6))],
                )?;
                Ok(())
            })
            .unwrap();
        let (status, _, body) = f.post(BASE, Some(&cookie), Some(ORIGIN_URL), request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.to_string()
                .contains("passkey_recent_authentication_required")
        );
        let mut revoke = f.expected_owner(Some(&cookie), None).await;
        revoke["issuance_id"] = issued["issuance_id"].clone();
        let (status, _, body) = f
            .post(
                &format!("{BASE}/revoke"),
                Some(&cookie),
                Some(ORIGIN_URL),
                revoke,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["revoked"], true);
        f.api
            .config_store
            .with_conn(|conn| {
                conn.execute("UPDATE browser_sessions SET revoked_at='revoked'", [])?;
                Ok(())
            })
            .unwrap();
        assert_ne!(
            f.get_authenticated(BASE, Some(&cookie), None).await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn invalid_expired_locked_and_revoked_codes_never_authorize_pending_ceremonies() {
        for invalidation in ["reissue", "revoke", "expiry", "attempts", "pending"] {
            let f = fixture().await;
            let (cookie, owner) = f
                .enroll("registration", None, "owner", &mut authenticator())
                .await;
            let issued = issue(&f, &cookie).await;
            let (status, _, body) = options(&f, &issued, None, TARGET).await;
            assert_eq!(status, StatusCode::OK);
            let request = credential(body, &mut authenticator());
            match invalidation {
                "reissue" => {
                    let newer = issue(&f,&cookie).await;
                    let mut revoke = f.expected_owner(Some(&cookie),None).await;
                    revoke["issuance_id"] = issued["issuance_id"].clone();
                    assert_eq!(f.post(&format!("{BASE}/revoke"),Some(&cookie),Some(ORIGIN_URL),revoke).await.2["revoked"],false);
                    assert_eq!(active(&f,&cookie).await["issuance_id"],newer["issuance_id"]);
                },
                "revoke" => {
                    let mut revoke = f.expected_owner(Some(&cookie),None).await;
                    revoke["issuance_id"] = issued["issuance_id"].clone();
                    assert_eq!(f.post(&format!("{BASE}/revoke"),Some(&cookie),Some(ORIGIN_URL),revoke).await.2["revoked"],true);
                },
                "expiry" => f.api.config_store.with_conn(|conn| {conn.execute("UPDATE passkey_addition_codes SET expires_at=?1",[rfc3339_after(Duration::seconds(-1))])?;Ok(())}).unwrap(),
                "attempts" => {
                    let mut wrong = issued.clone(); wrong["code"] = "wrong".into();
                    for _ in 0..5 { assert_eq!(options(&f,&wrong,None,TARGET).await.0,StatusCode::BAD_REQUEST); }
                    assert_eq!(options(&f,&issued,None,TARGET).await.0,StatusCode::BAD_REQUEST);
                },
                "pending" => f.api.config_store.with_conn(|conn| {conn.execute("INSERT INTO user_activations(user_id,issuance_id,code_hash,expires_at,failed_attempts) VALUES(?1,'pending','hash',?2,0)",rusqlite::params![owner.user_id,rfc3339_after(Duration::minutes(15))])?;Ok(())}).unwrap(),
                _ => unreachable!(),
            }
            let (status, headers, _) = complete(&f, request, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{invalidation}");
            assert!(!headers.contains_key(axum::http::header::SET_COOKIE));
            assert_eq!(f.credentials(&owner).len(), 1);
        }
    }

    #[tokio::test]
    async fn code_cannot_switch_cookie_owner_origin_or_registration_purpose() {
        let f = fixture().await;
        let (cookie, owner) = f
            .enroll("registration", None, "owner", &mut authenticator())
            .await;
        let (other, _) = f
            .enroll("registration", None, "other", &mut authenticator())
            .await;
        let issued = issue(&f, &cookie).await;
        assert_eq!(
            options(&f, &issued, Some(&other), TARGET).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            options(&f, &issued, None, ORIGIN_URL).await.0,
            StatusCode::BAD_REQUEST
        );
        let input =
            serde_json::json!({"handle":"owner","code":issued["code"],"browser_origin":TARGET});
        assert_eq!(
            f.post(
                &format!("{BASE}/options"),
                Some(&cookie),
                None,
                input.clone()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            f.post("/api/auth/activation/options", None, Some(TARGET), input)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        for route in [
            "/api/auth/activation/complete",
            "/api/auth/passkeys/registration/complete",
            "/api/auth/passkeys/addition/complete",
        ] {
            let (_, _, body) = options(&f, &issued, None, TARGET).await;
            let request = credential(body, &mut authenticator());
            assert_eq!(
                f.post(route, Some(&cookie), Some(TARGET), request).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        let (_, _, body) = options(&f, &issued, None, TARGET).await;
        let request = credential(body, &mut authenticator());
        assert_eq!(
            complete(&f, request, Some(&other)).await.0,
            StatusCode::BAD_REQUEST
        );
        let (_, _, body) = options(&f, &issued, None, TARGET).await;
        let request = credential(body, &mut authenticator());
        let mut removed = f.api.clone();
        let AuthConfig::Passkey { public_urls, .. } = &mut removed.config.auth;
        public_urls.retain(|url| url != TARGET);
        let app = generated_auth_contract_router(ServerApiContractService::Workspace(removed));
        assert_ne!(
            Fixture::post_to(
                app,
                &format!("{BASE}/complete"),
                None,
                None,
                Some(TARGET),
                request
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(f.credentials(&owner).len(), 1);
    }

    #[tokio::test]
    async fn canceled_failed_and_rolled_back_ceremonies_allow_fresh_options_but_concurrent_success_consumes_once()
     {
        let f = fixture().await;
        let (cookie, owner) = f
            .enroll("registration", None, "owner", &mut authenticator())
            .await;
        let issued = issue(&f, &cookie).await;
        let (_, _, canceled) = options(&f, &issued, None, TARGET).await;
        let (_, _, body) = options(&f, &issued, None, TARGET).await;
        let mut invalid = credential(body, &mut authenticator());
        invalid["challenge_id"] = canceled["challenge_id"].clone();
        assert_eq!(complete(&f, invalid, None).await.0, StatusCode::BAD_REQUEST);
        for table in ["passkey_credentials", "passkey_addition_codes"] {
            let (_, _, body) = options(&f, &issued, None, TARGET).await;
            let request = credential(body, &mut authenticator());
            f.api.config_store.with_conn(|conn| {conn.execute_batch(&format!("CREATE TRIGGER addition_code_failure BEFORE {} ON {table} BEGIN SELECT RAISE(ABORT,'injected'); END;",if table=="passkey_credentials" {"INSERT"} else {"DELETE"}))?;Ok(())}).unwrap();
            assert_eq!(
                complete(&f, request, None).await.0,
                StatusCode::INTERNAL_SERVER_ERROR
            );
            assert_eq!(f.credentials(&owner).len(), 1);
            assert_eq!(
                active(&f, &cookie).await["issuance_id"],
                issued["issuance_id"]
            );
            f.api
                .config_store
                .with_conn(|conn| {
                    conn.execute_batch("DROP TRIGGER addition_code_failure")?;
                    Ok(())
                })
                .unwrap();
        }
        let (_, _, a) = options(&f, &issued, None, TARGET).await;
        let (_, _, b) = options(&f, &issued, None, TARGET).await;
        let a = credential(a, &mut authenticator());
        let b = credential(b, &mut authenticator());
        let (a, b) = tokio::join!(complete(&f, a, None), complete(&f, b, None));
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
        assert_eq!(f.credentials(&owner).len(), 2);
        assert!(active(&f, &cookie).await.is_null());
    }
}
