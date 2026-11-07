//! Signed Session router -> Host handler -> retained Remote public reader.
//!
//! Signature/JTI verification, Worker catalog membership, Subject singleton,
//! policy, attribution, observation grants and SQLite authority are real. Only
//! the external Runtime data producer is a loopback fake. Its first successful
//! Read/Search response is held by an event; a second identical request receives
//! the same data at the same endpoint without replacing trust for fixture setup.
//! No authenticated context is constructed from a current binding lookup.
//!
//! List reads Host attribution/observation metadata and never calls Runtime. Its
//! queue cases pass the ORIGINAL real verifier result to the production handler;
//! positive controls additionally exercise the signed router. This suite does not
//! claim an Embedded Controller/Session-store execution: the Remote HTTP reply
//! hold has no equivalent in the synchronous Embedded public-reader path. The
//! shared production handler/authority fence is exercised, not a fake Embedded
//! Runtime registered under an Embedded identity. Existing archive/policy/body/
//! observation suites retain ownership of those distinct domain contracts.
//!
//! Normal registered lib tests, current-thread async callers, finite event waits
//! and failure-safe reply/task cleanup; no subprocess, sleeps or external model.
use super::*;
use server_api::{
    SubjektivSessionBackendOperation as Op, SubjektivSessionBackendRequest as SessionRequest,
    SubjektivSessionBackendResponse as SessionResponse,
    SubjektivSessionBackendResult as SessionResult,
};
use worker_runtime::auth::{RuntimeIdentityMaterial, RuntimeRequestSourceSigner};

const HISTORY: &str = "/api/w/db-execution/subjektiv/session-history";
const SECRET: &str = "protected needle from original Runtime reply";

#[derive(Clone, Copy, Debug)]
enum Change {
    Unchanged,
    Rotate,
    ReactivateSameKey,
    AbaSameTrust,
}

// Abort the async caller before fixture teardown on assertion/timeout. A running
// Host operation still owns its lease and the finite Runtime reply watchdog.
struct PendingRequest(tokio::task::JoinHandle<Response>);
impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct SignedSession {
    // Field drop order matters: unblock/join the producer before dropping DBs.
    reply: HeldRuntimeReply,
    fixture: Fixture,
    identity: RuntimeIdentityMaterial,
    subject: String,
    worker: RuntimeWorkerRef,
    bytes: Vec<u8>,
}
impl SignedSession {
    async fn new(kind: &str) -> Self {
        let fixture = Fixture::new().await;
        let reply = HeldRuntimeReply::with_response_count(None, Some(runtime_response(kind)), 2);
        let mut binding = install_remote_fixture(
            &fixture,
            "subject-runtime",
            &format!("http://{}", reply.address),
            false,
        );
        let identity = RuntimeIdentityMaterial::generate("subject-runtime").unwrap();
        binding.public_key = identity.public_key.clone();
        binding.public_key_fingerprint.clear();
        binding.authentication_mode =
            crate::store::WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity;
        fixture
            .store
            .upsert_workspace_runtime_binding(binding, true)
            .unwrap();
        // Setup uses the maintained Host reservation/registry/attribution fixture.
        // Its legacy synthetic inbound context is deliberately discarded here.
        // The outbound fake uses the fixture's permissive provider binding gate;
        // it cannot substitute for the signed inbound Session authority check.
        let (subject, worker, _) = session_runtime_subject(&fixture, kind).await;
        let operation = match kind {
            "read" => Op::Read(
                serde_json::from_value(serde_json::json!({
                    "session_id":"session", "segment_id":"segment", "entry_ref":"E1",
                    "mode":"compact"
                }))
                .unwrap(),
            ),
            "search" => Op::Search(
                serde_json::from_value(serde_json::json!({
                    "session_id":"session", "query":"needle"
                }))
                .unwrap(),
            ),
            "list" => Op::List(
                serde_json::from_value(serde_json::json!({"session_id":"session"})).unwrap(),
            ),
            _ => panic!("unknown Session operation: {kind}"),
        };
        let bytes = serde_json::to_vec(&SessionRequest { operation }).unwrap();
        Self {
            reply,
            fixture,
            identity,
            subject,
            worker,
            bytes,
        }
    }

    fn binding(&self) -> crate::store::WorkspaceRuntimeBinding {
        self.fixture
            .store
            .get_workspace_runtime_binding(WORKSPACE, &self.worker.runtime_id)
            .unwrap()
            .unwrap()
    }

    fn signed(&self) -> Request {
        let proof = RuntimeRequestSourceSigner::from_identity(&self.identity)
            .issue(
                self.fixture.api.config.backend_base_url.as_deref().unwrap(),
                WORKSPACE,
                Some(&self.worker.worker_id),
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                "POST",
                HISTORY,
                &self.bytes,
                i64::try_from(worker_runtime::auth::unix_now_seconds()).unwrap(),
                30,
            )
            .unwrap();
        Request::builder()
            .method("POST")
            .uri(HISTORY)
            .header(CONTENT_TYPE, "application/json")
            .header(
                worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER,
                proof,
            )
            .body(axum::body::Body::from(self.bytes.clone()))
            .unwrap()
    }

    fn dispatch(&self) -> PendingRequest {
        PendingRequest(self.fixture.dispatch(self.signed()))
    }

    async fn verified_context(&self) -> server_api::ServerRequestContext {
        let signed = self.signed();
        let proof = signed.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER]
            .to_str()
            .unwrap();
        let verified = observed(crate::worker_source::verify_runtime_request_source_proof(
            &self.fixture.api,
            proof,
            WORKSPACE,
            worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
            "POST",
            HISTORY,
            &worker_runtime::auth::request_body_digest(&self.bytes),
        ))
        .await
        .unwrap();
        assert_eq!(
            verified.worker_id.as_deref(),
            Some(self.worker.worker_id.as_str())
        );
        assert_eq!(
            verified.authentication_binding_id,
            self.binding().binding_id
        );
        server_api::ServerRequestContext {
            actor: None,
            worker_source: None,
            runtime_source: Some(server_api::ServerRuntimeSource {
                runtime_id: verified.runtime_id,
                worker_id: verified.worker_id,
                authentication_binding_id: verified.authentication_binding_id,
            }),
            origin: None,
            transport_headers: vec![],
        }
    }

    fn replace_trust(&mut self, change: Change) {
        if matches!(change, Change::Unchanged) {
            return;
        }
        let original = self.binding();
        let mut replacement = original.clone();
        match change {
            Change::Unchanged => unreachable!(),
            Change::Rotate => {
                self.identity = RuntimeIdentityMaterial::generate(&self.worker.runtime_id).unwrap();
                replacement.public_key = self.identity.public_key.clone();
                replacement.public_key_fingerprint.clear();
            }
            Change::ReactivateSameKey => {
                assert!(
                    self.fixture
                        .store
                        .revoke_workspace_runtime_binding(
                            WORKSPACE,
                            &self.worker.runtime_id,
                            "2026-01-01T00:00:00Z"
                        )
                        .unwrap()
                );
            }
            Change::AbaSameTrust => {
                // A -> different signing key -> A, with the original endpoint,
                // metadata and public key restored but never the original binding.
                let temporary = RuntimeIdentityMaterial::generate(&self.worker.runtime_id).unwrap();
                replacement.public_key = temporary.public_key.clone();
                replacement.public_key_fingerprint.clear();
                self.fixture
                    .store
                    .upsert_workspace_runtime_binding(replacement, true)
                    .unwrap();
                assert_ne!(self.binding().public_key, original.public_key);
                assert_ne!(self.binding().binding_id, original.binding_id);
                replacement = original.clone();
            }
        }
        self.fixture
            .store
            .upsert_workspace_runtime_binding(replacement, true)
            .unwrap();
        let current = self.binding();
        assert_ne!(current.binding_id, original.binding_id);
        assert!(current.revoked_at.is_none());
        assert_eq!(current.public_key, self.identity.public_key);
        if !matches!(change, Change::Rotate) {
            assert_eq!(current.public_key, original.public_key);
            assert_eq!(
                current.public_key_fingerprint,
                original.public_key_fingerprint
            );
        }
        if matches!(change, Change::AbaSameTrust) {
            assert_eq!(current.display_name, original.display_name);
            assert_eq!(current.base_url, original.base_url);
            assert_eq!(current.authentication_mode, original.authentication_mode);
        }
    }

    // These are protected domain records, not trust or the verifier's one-use
    // JTIs. Read operations have no operation/idempotency key to consume/retry.
    fn domain_snapshot(&self) -> serde_json::Value {
        let store = open_subjektiv_store(&self.fixture.api).unwrap();
        serde_json::json!({
            "policy": store.current_subject_policy(&self.subject).unwrap(),
            "attribution": store.subject_session_attributions(&self.subject).unwrap(),
            "owner": self.fixture.store.require_current_worker_singleton_owner(
                WORKSPACE, &self.worker
            ).unwrap(),
            "worker": self.fixture.store.get_worker_registry(WORKSPACE, &self.worker).unwrap(),
            "observe": self.fixture.store.get_active_worker_control_grant(
                WORKSPACE, &self.worker, &self.worker
            ).unwrap(),
            "archive": self.fixture.store.worker_session_archive(
                WORKSPACE, &self.worker, "session"
            ).unwrap(),
        })
    }
}

fn runtime_response(kind: &str) -> serde_json::Value {
    let mut entry = serde_json::json!({
        "segment_id":"segment", "entry_ref":"E1", "kind":"user",
        "origin":"human_input", "lineage":{"kind":"root"}
    });
    let response = if kind == "read" {
        entry["session_id"] = "session".into();
        entry["generation"] = "generation".into();
        entry["mode"] = "compact".into();
        entry["content"] = SECRET.into();
        entry["has_more"] = false.into();
        entry["scanned_bytes"] = 64.into();
        entry["scanned_segments"] = 1.into();
        entry["scanned_entries"] = 1.into();
        serde_json::json!({"availability":"page", "page":entry})
    } else {
        entry["compact"] = SECRET.into();
        entry["compact_truncated"] = false.into();
        entry["scan_cursor"] = "start".into();
        serde_json::json!({"availability":"page", "page":{
            "session_id":"session", "generation":"generation", "items":[entry],
            "has_more":false, "scanned_bytes":64, "scanned_segments":1, "scanned_entries":1
        }})
    };
    // Validate the fake at its real protocol boundary before any scheduling.
    if kind == "read" {
        serde_json::from_value::<runtime_api::SessionPublicReadAvailability>(response.clone())
            .unwrap();
    } else {
        serde_json::from_value::<runtime_api::SessionPublicSearchAvailability>(response.clone())
            .unwrap();
    }
    response
}

async fn response_body(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = observed(axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024))
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn assert_positive(kind: &str, body: serde_json::Value) {
    let SessionResponse::Ok { result } = serde_json::from_value(body.clone()).unwrap() else {
        panic!("{kind} must return protected data, not a diagnostic: {body}")
    };
    match (kind, result) {
        ("read", SessionResult::Read(page)) => {
            assert_eq!(page.session_id, "session");
            assert_eq!(page.entry_ref, "E1");
            assert_eq!(page.content, SECRET);
            assert!(!page.truncated);
        }
        ("search", SessionResult::Search(page)) => {
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].session_id, "session");
            assert_eq!(page.items[0].entry_ref, "E1");
            assert_eq!(page.items[0].snippet, SECRET);
            assert!(!page.items[0].snippet_truncated);
            assert!(page.issues.is_empty());
        }
        ("list", SessionResult::List(page)) => {
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].session_id, "session");
            assert_eq!(
                page.items[0].storage,
                server_api::SubjektivSessionStorage::Retained
            );
            assert_eq!(
                page.items[0].availability,
                server_api::SubjektivSessionAvailability::Unchecked
            );
        }
        (_, result) => panic!("unexpected {kind} response: {result:?}"),
    }
}

async fn held_reply_case(kind: &str, change: Change) {
    let mut f = SignedSession::new(kind).await;
    let before = f.domain_snapshot();
    let original = f.binding();
    let mut call = f.dispatch();
    // Dispatch cannot reach this external request without real signature/body/
    // Workspace/Worker verification, context projection and handler admission.
    let request = observed(f.reply.entered.take().unwrap()).await.unwrap();
    assert!(request.contains("/v1/session-public/"), "{request}");
    assert!(request.contains(kind), "{request}");
    assert_eq!(f.binding().binding_id, original.binding_id);
    assert!(!f.reply.upstream_released.load(Ordering::SeqCst));
    assert!(
        !call.0.is_finished(),
        "must still be held before trust changes"
    );
    f.replace_trust(change);
    assert_eq!(f.domain_snapshot(), before, "only Runtime trust changed");
    f.reply.release.take().unwrap().send(()).unwrap();
    let response = observed(&mut call.0).await.unwrap();
    let (status, body) = response_body(response).await;
    if matches!(change, Change::Unchanged) {
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_positive(kind, body);
    } else {
        assert_eq!(status, StatusCode::FORBIDDEN, "{kind}/{change:?}: {body}");
        let text = body.to_string();
        assert!(text.contains("authentication binding changed"), "{text}");
        assert!(
            !text.contains(SECRET),
            "stale request leaked protected content/snippet"
        );
        assert!(
            serde_json::from_value::<SessionResponse>(body).is_err(),
            "stale request must not receive a successful protected DTO"
        );
    }
    assert_eq!(f.domain_snapshot(), before, "Session reads are effect-free");

    // Same exact request bytes/Worker/selector, freshly genuinely signed using
    // current trust (B2 after a change). A proof JTI itself is never replayed.
    let current_binding = f.binding().binding_id;
    let mut current = f.dispatch();
    let (status, body) = response_body(observed(&mut current.0).await.unwrap()).await;
    assert_eq!(status, StatusCode::OK, "fresh current trust: {body}");
    assert_positive(kind, body);
    assert_eq!(f.binding().binding_id, current_binding);
    assert_eq!(f.domain_snapshot(), before);
    f.reply.finish();
    observed(f.fixture.host_executor.drain()).await.unwrap();
    observed(f.fixture.executor.drain()).await.unwrap();
}

macro_rules! signed_reply_case {
    ($name:ident, $kind:literal, $change:ident) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            held_reply_case($kind, Change::$change).await;
        }
    };
}
signed_reply_case!(signed_read_unchanged_returns_content, "read", Unchanged);
signed_reply_case!(signed_search_unchanged_returns_snippet, "search", Unchanged);
signed_reply_case!(signed_read_rotation_withholds_content, "read", Rotate);
signed_reply_case!(signed_search_rotation_withholds_snippet, "search", Rotate);
signed_reply_case!(
    signed_read_same_key_reactivation_withholds_content,
    "read",
    ReactivateSameKey
);
signed_reply_case!(
    signed_search_same_key_reactivation_withholds_snippet,
    "search",
    ReactivateSameKey
);
signed_reply_case!(
    signed_read_aba_same_key_withholds_content,
    "read",
    AbaSameTrust
);
signed_reply_case!(
    signed_search_aba_same_key_withholds_snippet,
    "search",
    AbaSameTrust
);

#[tokio::test(flavor = "current_thread")]
async fn signed_list_queue_retains_original_verified_binding_and_current_list_is_read_only() {
    for change in [
        Change::Unchanged,
        Change::Rotate,
        Change::ReactivateSameKey,
        Change::AbaSameTrust,
    ] {
        let mut f = SignedSession::new("list").await;
        let before = f.domain_snapshot();
        let context = f.verified_context().await;
        let original = context
            .runtime_source
            .as_ref()
            .unwrap()
            .authentication_binding_id
            .clone();
        let (entered, started) = oneshot::channel();
        let (resume, release) = mpsc::channel();
        let executor = f.fixture.host_executor.clone();
        let mut occupying = Box::pin(executor.run(move || -> crate::Result<()> {
            let _ = entered.send(());
            release
                .recv_timeout(WATCHDOG)
                .expect("release Session Host executor");
            Ok(())
        }));
        assert!(futures::poll!(occupying.as_mut()).is_pending());
        observed(started).await.unwrap();
        let mut queued = Box::pin(scoped_subjektiv_session_backend(
            State(f.fixture.api.clone()),
            AxumPath(ScopedWorkspacePath {
                workspace_id: WORKSPACE.into(),
            }),
            context,
            Json(serde_json::from_slice::<SessionRequest>(&f.bytes).unwrap()),
        ));
        // Exactly one occupied Host worker: this poll actually enqueues the
        // verified production handler, rather than pausing before submission.
        assert!(futures::poll!(queued.as_mut()).is_pending());
        f.replace_trust(change);
        if !matches!(change, Change::Unchanged) {
            assert_ne!(f.binding().binding_id, original);
        }
        resume.send(()).unwrap();
        observed(occupying).await.unwrap();
        let result = observed(queued).await;
        if matches!(change, Change::Unchanged) {
            assert_positive("list", serde_json::to_value(result.unwrap().0).unwrap());
        } else {
            let error = result.unwrap_err();
            assert!(
                matches!(error.error, Error::WorkspacePermissionDenied(_)),
                "{error:?}"
            );
            assert!(
                error
                    .error
                    .to_string()
                    .contains("authentication binding changed"),
                "{error:?}"
            );
        }
        assert_eq!(f.domain_snapshot(), before);
        let mut current = f.dispatch();
        let (status, body) = response_body(observed(&mut current.0).await.unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_positive("list", body);
        assert_eq!(f.domain_snapshot(), before);
        assert!(
            matches!(
                f.reply.entered.as_mut().unwrap().try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "List must not call Runtime"
        );
        f.reply.finish();
        observed(f.fixture.host_executor.drain()).await.unwrap();
        observed(f.fixture.executor.drain()).await.unwrap();
    }
}
