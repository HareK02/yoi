//! Cross-lane composition regressions for T743's three new async DB callers.
//! Domain success/CAS/replay/rollback is covered by assignment_tracking_tests;
//! these exercise real generated handlers and their finite execution owners.
use super::*;

struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.take();
    }
}

async fn queued_caller_keeps_owner(
    method: &str,
    suffix: &str,
    body: serde_json::Value,
    host: bool,
) {
    let fixture = Fixture::new().await;
    let executor = if host {
        fixture.host_executor.clone()
    } else {
        fixture.executor.clone()
    };
    let store = fixture.store.clone();
    let (entered, waiting) = oneshot::channel();
    let (release, permit) = mpsc::channel();
    let mut release = Release(Some(release));
    let watchdog = Arc::new(AtomicBool::new(false));
    let expired = watchdog.clone();
    // Occupy the actual owner and connection deterministically, not via sleeps
    // or a request that could still be waiting in its authentication stage.
    executor
        .dispatch(move || {
            store
                .with_conn(|_| {
                    let _ = entered.send(());
                    if permit.recv_timeout(WATCHDOG).is_err() {
                        expired.store(true, Ordering::SeqCst);
                    }
                    Ok(())
                })
                .unwrap();
        })
        .unwrap();
    observed(waiting).await.unwrap();
    let path = format!("/api/w/{WORKSPACE}/tickets/T-missing/{suffix}");
    let request = || Fixture::request(method, &path, body.clone());
    let (lifetime, accepted) = admission_observation();
    let mut queued = Box::pin(crate::db_executor::with_operation_lifetime(
        lifetime,
        fixture.inner_router().oneshot(request()),
    ));
    assert!(futures::poll!(&mut queued).is_pending());
    observed(accepted).await.unwrap();
    let control = tokio::spawn(async { "independent control" });
    assert_eq!(observed(control).await.unwrap(), "independent control");
    observed(tokio::time::sleep(StdDuration::from_millis(1))).await;
    assert!(
        !watchdog.load(Ordering::SeqCst),
        "synchronous caller stalled Tokio until the watchdog"
    );
    drop(queued);
    // Cancellation of the waiter cannot free the accepted queued SQL slot.
    let rejected = observed(fixture.inner_router().oneshot(request()))
        .await
        .unwrap();
    let error = response_json(rejected, StatusCode::SERVICE_UNAVAILABLE).await;
    assert!(
        error["message"].as_str().unwrap().contains("capacity"),
        "{error}"
    );
    let mut drained = Box::pin(executor.drain());
    assert!(futures::poll!(&mut drained).is_pending());
    release.0.take().unwrap().send(()).unwrap();
    observed(drained).await.unwrap();
    // The accepted request finishes its real lookup after cancellation; the
    // normal domain error and reusable capacity survive without SQL resending.
    let after = observed(fixture.inner_router().oneshot(request()))
        .await
        .unwrap();
    response_json(after, StatusCode::NOT_FOUND).await;
    observed(executor.drain()).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn assignments_page_uses_finite_db_owner_and_survives_cancel() {
    queued_caller_keeps_owner(
        "GET",
        "assignment-tracking/assignments",
        serde_json::json!({}),
        false,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn diagnostics_page_uses_finite_db_owner_and_survives_cancel() {
    queued_caller_keeps_owner(
        "GET",
        "assignment-diagnostics?assignment_id=missing",
        serde_json::json!({}),
        false,
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn answer_uses_finite_host_owner_and_survives_cancel() {
    queued_caller_keeps_owner(
        "POST",
        "assignment-answer",
        serde_json::json!({
            "assignment_id":"missing", "expected_last_event_id":"missing",
            "operation_key":"answer-probe", "body":"continuing input"
        }),
        true,
    )
    .await;
}
