// Generated API + real SQLite/token/browser authorization + loopback Workspace WS.
// Runtime execution alone is supplied by the shared isolated test_api fixture.
mod workspace_membership_api {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use protocol::subscription::{
        EventSubscriptionSelector, SubscriptionFrame, SubscriptionFramePayload, SubscriptionRequest,
    };
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{Message, client::IntoClientRequest},
    };

    struct Fixture {
        _dir: tempfile::TempDir,
        api: WorkspaceApi,
        app: Router,
        owner: String,
        member: String,
        outsider: String,
    }
    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let api = test_api(dir.path()).await;
            let owner = seed_test_owner_api_token(&api, "membership-owner");
            let member = seed_test_api_token(api.store.as_ref(), "membership-member");
            let outsider = seed_test_api_token(api.store.as_ref(), "membership-outsider");
            let catalog = WorkspaceServerApi::new(api.config.clone(), api.store.clone());
            catalog
                .apis
                .lock()
                .await
                .insert(TEST_WORKSPACE_ID.into(), api.clone());
            catalog
                .routers
                .lock()
                .await
                .insert(TEST_WORKSPACE_ID.into(), build_inner_router(api.clone()));
            Self {
                _dir: dir,
                api,
                app: workspace_server_router(catalog),
                owner,
                member,
                outsider,
            }
        }
        fn path(&self, resource: &str) -> String {
            format!("/api/w/{TEST_WORKSPACE_ID}/{resource}")
        }
        async fn request(
            &self,
            token: Option<&str>,
            method: &str,
            path: &str,
            body: Option<Value>,
        ) -> (StatusCode, Value) {
            let mut req = Request::builder()
                .method(method)
                .uri(path)
                .header(CONTENT_TYPE, "application/json");
            if let Some(token) = token {
                req = req.header("authorization", format!("Bearer {token}"));
            }
            let res = self
                .app
                .clone()
                .oneshot(
                    req.body(
                        body.map(|v| Body::from(v.to_string()))
                            .unwrap_or_else(Body::empty),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            let status = res.status();
            let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            )
        }
        async fn add(&self) -> Value {
            let (status, list) = self
                .request(Some(&self.owner), "GET", &self.path("members"), None)
                .await;
            assert_eq!(status, StatusCode::OK, "{list}");
            let (status,result)=self.request(Some(&self.owner),"POST",&self.path("members"),Some(json!({"operation_key":"add-member","account_handle":"user-membership-member","expected_membership_change_key":list["membership_change_key"]}))).await;
            assert_eq!(status, StatusCode::OK, "{result}");
            result
        }
        async fn remove(&self, key: &Value) {
            let (status, result) = self
                .request(
                    Some(&self.owner),
                    "DELETE",
                    &self.path("members/account-membership-member"),
                    Some(json!({"operation_key":"remove-member","expected_membership_key":key})),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{result}");
        }
    }

    #[tokio::test]
    async fn journal_read_post_requires_current_member_and_never_grants_owner_writes() {
        let f = Fixture::new().await;
        let subject = open_subjektiv_store(&f.api)
            .unwrap()
            .create_subject(crate::subjektiv::SubjectRole::new("member-journal").unwrap())
            .unwrap();
        let path = f.path(&format!(
            "subjektiv/subjects/{}/journal-browser",
            subject.id
        ));
        let read = json!({"action":"sessions","after":null,"limit":10});
        assert_eq!(
            f.request(None, "POST", &path, Some(read.clone())).await.0,
            StatusCode::UNAUTHORIZED
        );
        for token in [&f.member, &f.outsider] {
            assert_eq!(
                f.request(Some(token), "POST", &path, Some(read.clone()))
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
        }
        let added = f.add().await;
        for token in [&f.owner, &f.member] {
            let (status, body) = f
                .request(Some(token), "POST", &path, Some(read.clone()))
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        for bad in [
            json!({"action":"delete","journal_id":"x"}),
            json!({"action":"sessions","after":null,"limit":10,"write":true}),
            json!({"action":"sessions","after":null,"limit":100000}),
        ] {
            assert!(
                f.request(Some(&f.member), "POST", &path, Some(bad))
                    .await
                    .0
                    .is_client_error()
            );
        }
        assert_eq!(
            f.request(Some(&f.member), "PUT", &f.path("flows"), Some(json!({})))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let cross = path.replace(TEST_WORKSPACE_ID, "other-workspace");
        assert!(
            f.request(Some(&f.member), "POST", &cross, Some(read.clone()))
                .await
                .0
                .is_client_error()
        );
        f.remove(&added["membership_key"]).await;
        assert_eq!(
            f.request(Some(&f.member), "POST", &path, Some(read))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn membership_api_denies_nonmembers_filters_catalog_and_separates_owner_settings() {
        let f = Fixture::new().await;
        let admin = f
            .api
            .config_store
            .setup_hosting_admin("membership-admin", true)
            .unwrap()
            .initial_bearer
            .unwrap()
            .access_token;
        for path in [
            f.path("workspace"),
            f.path("tickets"),
            f.path("objectives"),
            f.path("members"),
            "/api/workspace".into(),
            "/api/tickets".into(),
            f.path("drive/root"),
            f.path("runtimes/missing/workers/missing/sessions/missing/attachments/missing"),
        ] {
            assert_eq!(
                f.request(None, "GET", &path, None).await.0,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            for token in [&f.member, &f.outsider, &admin] {
                assert_eq!(
                    f.request(Some(token), "GET", &path, None).await.0,
                    StatusCode::FORBIDDEN,
                    "{path}"
                );
            }
        }
        for token in [&f.member, &f.outsider, &admin] {
            let (status, body) = f.request(Some(token), "GET", "/api/workspaces", None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, json!([]));
        }
        let add = f.add().await;
        let (status, list) = f
            .request(Some(&f.member), "GET", "/api/workspaces", None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["workspace_id"], TEST_WORKSPACE_ID);
        for path in [
            f.path("workspace"),
            f.path("tickets"),
            f.path("objectives"),
            "/api/workspace".into(),
        ] {
            assert_eq!(
                f.request(Some(&f.member), "GET", &path, None).await.0,
                StatusCode::OK,
                "{path}"
            );
        }
        for path in [
            f.path("members"),
            f.path("settings"),
            f.path("settings/memory"),
            f.path("config/source-tree"),
        ] {
            assert_eq!(
                f.request(Some(&f.member), "GET", &path, None).await.0,
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        // Reading configuration stays member-facing; authoring is owner-only.
        assert_eq!(
            f.request(Some(&f.member), "GET", &f.path("flows"), None)
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            f.request(Some(&f.member), "PUT", &f.path("flows"), Some(json!({})))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let (status,body)=f.request(Some(&f.member),"POST",&f.path("ticket-tags"),Some(json!({"operation_key":"member-tag","action":{"kind":"create","name":"member-label"}}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status,body)=f.request(Some(&f.owner),"POST",&f.path("members"),Some(json!({"operation_key":"missing-cas","account_handle":"user-membership-outsider"}))).await;
        assert!(status.is_client_error(), "{body}");
        f.remove(&add["membership_key"]).await;
        assert_eq!(
            f.request(Some(&f.member), "GET", &f.path("workspace"), None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let (status, list) = f
            .request(Some(&f.member), "GET", "/api/workspaces", None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list, json!([]));
        assert_eq!(
            f.request(Some(&f.owner), "GET", &f.path("workspace"), None)
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn membership_browser_session_revocation_and_cross_workspace_access_are_enforced() {
        let f = Fixture::new().await;
        let added = f.add().await;
        f.api
            .store
            .create_browser_session(&BrowserSessionRecord {
                token_hash: crate::auth::token_hash("member-browser-session"),
                session_id: "membership-browser".into(),
                user_id: "user-membership-member".into(),
                created_at: crate::auth::now_rfc3339(),
                expires_at: "2099-01-01T00:00:00Z".into(),
                revoked_at: None,
            })
            .unwrap();
        let AuthConfig::Passkey { cookie_name, .. } = &f.api.config.auth;
        let request = || {
            Request::builder()
                .uri(f.path("workspace"))
                .header("cookie", format!("{cookie_name}=member-browser-session"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            f.app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::OK
        );
        let other = Uuid::now_v7().to_string();
        f.api
            .store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: other.clone(),
                owner_account_id: "account-membership-outsider".into(),
                display_name: "Other".into(),
                state: "active".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .await
            .unwrap();
        assert_eq!(
            f.request(
                Some(&f.member),
                "GET",
                &format!("/api/w/{other}/workspace"),
                None
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        let owner = f
            .api
            .store
            .get_workspace(TEST_WORKSPACE_ID)
            .await
            .unwrap()
            .unwrap()
            .owner_account_id;
        assert_eq!(
            f.request(
                Some(&f.owner),
                "DELETE",
                &f.path(&format!("members/{owner}")),
                Some(
                    json!({"operation_key":"remove-owner","expected_membership_key":"not-a-grant"})
                )
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(f.request(Some(&f.owner),"POST",&f.path("members"),Some(json!({"operation_key":"missing-account","account_handle":"does-not-exist","expected_membership_change_key":added["membership_change_key"]}))).await.0,StatusCode::NOT_FOUND);
        assert_eq!(f.request(Some(&f.owner),"POST",&f.path("members"),Some(json!({"operation_key":"stale","account_handle":"user-membership-outsider","expected_membership_change_key":null}))).await.0,StatusCode::CONFLICT);
        f.remove(&added["membership_key"]).await;
        assert_eq!(
            f.app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let (status,replay)=f.request(Some(&f.owner),"POST",&f.path("members"),Some(json!({"operation_key":"add-member","account_handle":"user-membership-member","expected_membership_change_key":null}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(replay["replayed"], true);
        assert_eq!(
            f.app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    fn subscribe(id: &str) -> Message {
        Message::Text(
            serde_json::to_string(&SubscriptionFrame::new(SubscriptionFramePayload::Request(
                SubscriptionRequest::SubscribeEvents {
                    request_id: protocol::subscription::SubscriptionRequestId::new(id).unwrap(),
                    selector: EventSubscriptionSelector::WorkspaceWorkers,
                },
            )))
            .unwrap()
            .into(),
        )
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn membership_websocket_revocation_stops_existing_delivery_commands_and_reconnect() {
        let f = Fixture::new().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = f.app.clone();
        let _server = AuthorizationCallbackServer(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let url = format!("ws://{address}{}", f.path("protocol/ws"));
        let request = |token: &str| {
            let mut req = url.clone().into_client_request().unwrap();
            req.headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().unwrap());
            req
        };
        for token in [&f.member, &f.outsider] {
            let error = connect_async(request(token)).await.unwrap_err();
            let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
                panic!("expected HTTP denial")
            };
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let add = f.add().await;
        let (mut owner, _) = connect_async(request(&f.owner)).await.unwrap();
        let (mut member, _) = connect_async(request(&f.member)).await.unwrap();
        let (mut commands, _) = connect_async(request(&f.member)).await.unwrap();
        owner.send(subscribe("owner-initial")).await.unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), owner.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        member.send(subscribe("initial")).await.unwrap();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), member.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(frame, Message::Text(_)));
        f.remove(&add["membership_key"]).await;
        // New requests on the admitted connection cannot produce another protected snapshot.
        let _ = commands.send(subscribe("after-revoke")).await;
        let command_response =
            tokio::time::timeout(std::time::Duration::from_secs(10), commands.next())
                .await
                .unwrap();
        assert!(!matches!(command_response, Some(Ok(Message::Text(_)))));
        seed_cleanup_worker(&f.api, 42, "normal");
        f.api
            .worker_projection
            .refresh_async(&RuntimeWorkerRef::new("runtime-test", "42"))
            .await
            .unwrap();
        let next = tokio::time::timeout(std::time::Duration::from_secs(10), member.next())
            .await
            .expect("revoked connection closes without reconnect");
        assert!(
            !matches!(next, Some(Ok(Message::Text(_)))),
            "revoked subscription delivered data: {next:?}"
        );
        assert!(connect_async(request(&f.member)).await.is_err());
        // A committed event reaches the owner but not the revoked, otherwise idle subscriber.
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), owner.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(frame, Message::Text(_)));
        let _ = owner.close(None).await;
    }
}
