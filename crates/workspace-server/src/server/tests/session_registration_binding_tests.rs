//! Signed Session registration retains request trust at all three native writes.
//! Attribution, writer claim and Session binding are separately committed stages:
//! a stage authorized before rotation remains historical, never rollback proof.
//! Embedded/Remote fixtures and bounded parent execution are shared with Memory;
//! these checks assert storage effects, not Session content or model processing.
use super::*;

macro_rules! registration_case {
    ($parent:ident, $child:ident, $kind:ident, $change:ident) => {
        #[test]
        fn $parent() {
            bounded(concat!("registration::", stringify!($child)));
        }
        #[test]
        #[ignore = "exact subprocess case owned by bounded signed registration parent"]
        fn $child() {
            with_runtime(registration(RuntimeKind::$kind, Change::$change));
        }
    };
}
registration_case!(
    signed_registration_embedded_current_commits,
    embedded_current_case,
    Embedded,
    Unchanged
);
registration_case!(
    signed_registration_remote_current_commits,
    remote_current_case,
    Remote,
    Unchanged
);
registration_case!(
    signed_registration_embedded_rotation_fences_every_write,
    embedded_rotation_case,
    Embedded,
    Rotate
);
registration_case!(
    signed_registration_remote_rotation_fences_every_write,
    remote_rotation_case,
    Remote,
    Rotate
);
registration_case!(
    signed_registration_embedded_reactivation_fences_every_write,
    embedded_reactivation_case,
    Embedded,
    ReactivateSameKey
);
registration_case!(
    signed_registration_remote_reactivation_fences_every_write,
    remote_reactivation_case,
    Remote,
    ReactivateSameKey
);
registration_case!(
    signed_registration_embedded_aba_fences_every_write,
    embedded_aba_case,
    Embedded,
    AbaSameTrust
);
registration_case!(
    signed_registration_remote_aba_fences_every_write,
    remote_aba_case,
    Remote,
    AbaSameTrust
);

async fn registration(kind: RuntimeKind, change: Change) {
    let mut f = SignedMemory::new(kind).await;
    for stage in 1..=3 {
        let session = session_store::new_session_id().to_string();
        let request = Wire {
            route: "sessions",
            operation: "session.record",
            bytes: serde_json::to_vec(&json!({"session_id":session,"create_if_missing":true}))
                .unwrap(),
        };
        let before = f.snapshot();
        let original_worker = f.authority_projection(&f.policy.source);
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let entered = Mutex::new(Some(entered));
        let (resume, release) = mpsc::channel();
        let release = Mutex::new(release);
        let calls = AtomicUsize::new(0);
        let mut api = f.policy.api.clone();
        api.subject_effect_admission_observer = Some(Arc::new(move |operation| {
            if operation == "session.record" && calls.fetch_add(1, Ordering::SeqCst) + 1 == stage {
                entered.lock().unwrap().take().unwrap().send(()).unwrap();
                release
                    .lock()
                    .unwrap()
                    .recv_timeout(WAIT)
                    .expect("release native registration stage");
            }
        }));
        let signed = f.signed(&f.policy.source, &request);
        let call = tokio::spawn(async move { response(&api, signed).await });
        tokio::time::timeout(WAIT, waiting).await.unwrap().unwrap();
        // The real router has verified B1 and reached this stage's native Feature
        // transaction. No Host/registry reservation is held while changing trust.
        f.replace_trust(change);
        resume.send(()).unwrap();
        let (status, body) = tokio::time::timeout(WAIT, call).await.unwrap().unwrap();
        assert_eq!(f.authority_projection(&f.policy.source), original_worker);
        if matches!(change, Change::Unchanged) {
            assert_eq!(status, StatusCode::OK, "stage{stage}: {body}");
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "stage{stage}: {body}");
            let store = open_subjektiv_store(&f.policy.api).unwrap();
            assert_eq!(
                store.session_attribution(&session).unwrap().is_some(),
                stage > 1,
                "only attribution authorized BEFORE the held stage may remain"
            );
            assert_eq!(
                binding_count(&f, &session),
                0,
                "stale callback cannot finish native binding"
            );
            if stage == 1 {
                assert_eq!(
                    f.snapshot(),
                    before,
                    "refusal at first effect leaves no attribution/writer/receipt"
                );
            }
            // The exact Session identity remains reusable by a genuinely signed
            // B2 request. No new operation identity or fabricated proof is issued.
            f.call_ok(&f.policy.source, &request).await;
        }
        assert_eq!(binding_count(&f, &session), 1);
        let saved = f.snapshot();
        f.call_ok(&f.policy.source, &request).await;
        assert_eq!(
            f.snapshot(),
            saved,
            "current exact Session replay is effect-free"
        );
    }
    f.policy.api.stop_backend_job_recovery().await;
}

fn binding_count(f: &SignedMemory, session: &str) -> i64 {
    f.policy
        .api
        .feature_storage
        .open(&f.policy.api.subjektiv_registration)
        .unwrap()
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT count(*) FROM journal_session_bindings WHERE session_id=?1",
                [session],
                |row| row.get(0),
            )?)
        })
        .unwrap()
}
