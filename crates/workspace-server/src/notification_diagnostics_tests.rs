// Production Store and SQLite. No Runtime/model side effects are required for diagnostics.
#[tokio::test]
async fn notification_diagnostics_preserves_unknown_acceptance_and_marks_replacement_ambiguity() {
    use server_api::{
        NotificationDiagnosticStage as S, NotificationDiagnosticsQuery as Q,
        NotificationTargetResolution as T,
    };
    for initial_failure in [
        NotificationFailureCode::AcceptanceUnknown,
        NotificationFailureCode::AcknowledgementFailed,
    ] {
        let (_dir, store) = tracking_store_fixture().await;
        tracking_store_assign(&store, "a", "assign-a");
        let source = tracking_store_source();
        let delivery = delivery_store_reserve_initial(&store, &source);
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &delivery.delivery_id,
                "frozen body",
                &source,
            )
            .unwrap();
        let key = NotificationDeliveryKey::AssignmentInput {
            assignment_id: "a".into(),
            input_event_id: delivery.input_event_id.clone(),
        };
        store
            .record_notification_delivery_failure("w", &key, initial_failure)
            .unwrap();
        store
            .record_notification_delivery_failure("w", &key, NotificationFailureCode::TargetStopped)
            .unwrap();
        let query = Q {
            delivery_id: Some(delivery.delivery_id.clone()),
            ..Default::default()
        };
        let read = || {
            store
                .notification_diagnostics("w", &query)
                .unwrap()
                .items
                .remove(0)
        };
        assert_eq!(read().stage, S::AcceptanceUnknown);
        assert!(read().acceptance_outcome_unknown);
        assert_eq!(read().last_failure_code.as_deref(), Some("target_stopped"));
        store.with_conn(|conn| {
            conn.execute_batch("UPDATE worker_create_reservations SET state='removed' WHERE workspace_id='w' AND singleton_key='subjektiv:tracking-subject';
                UPDATE worker_singleton_owners SET worker_id='replacement',generation=2 WHERE workspace_id='w' AND singleton_key='subjektiv:tracking-subject';
                INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at,singleton_key,singleton_generation)
                VALUES('w','replacement-body','replacement','r','replacement-body','created','2','2','subjektiv:tracking-subject',2);")?;
            Ok(())
        }).unwrap();
        let replacement = TicketAssignmentTrackingSourceContext {
            worker_id: "replacement".into(),
            ..source
        };
        store
            .prepare_ticket_assignment_delivery_content(
                "w",
                &delivery.delivery_id,
                "ignored new body",
                &replacement,
            )
            .unwrap();
        let ambiguous = read();
        assert_eq!(ambiguous.target_resolution, T::MultipleAuthorizedTargets);
        assert!(ambiguous.target_runtime_id.is_none() && ambiguous.target_worker_id.is_none());
        store
            .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &replacement)
            .unwrap();
        let accepted = read();
        assert_eq!(accepted.target_resolution, T::RuntimeAccepted);
        assert_eq!(accepted.target_worker_id.as_deref(), Some("replacement"));
        assert!(!accepted.acceptance_outcome_unknown);
    }
}

#[tokio::test]
async fn notification_diagnostics_detects_exact_missing_mr_event_after_earlier_capture() {
    use server_api::NotificationDiagnosticsQuery as Q;
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO merge_requests(workspace_id,merge_request_id,repository_id,state,selector_to,created_at,updated_at) SELECT workspace_id,'mr-diagnostic',repository_id,'open','develop','1','1' FROM repositories WHERE workspace_id='w' LIMIT 1",[])?;
        for (event,seq) in [("first",1),("missing",2)] {
            conn.execute("INSERT INTO merge_request_thread_events(workspace_id,merge_request_id,event_id,sequence,kind,payload_json,created_at) VALUES('w','mr-diagnostic',?1,?2,'review','{}','1')",params![event,seq])?;
        }
        conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) SELECT workspace_id,ticket_id,'mr-event:mr-diagnostic:first',role,assignment_id,'mr_review',json_set(facts_json,'$.merge_request_id','mr-diagnostic') FROM notification_sources WHERE assignment_id='a' LIMIT 1",[])?;
        Ok(())
    }).unwrap();
    let page = store
        .notification_diagnostics(
            "w",
            &Q {
                merge_request_id: Some("mr-diagnostic".into()),
                source_event_id: Some("mr-event:mr-diagnostic:missing".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(page.items.is_empty());
    assert!(page.source_checks[0].captured);
    assert_eq!(page.source_checks[1].origin_present, Some(true));
    assert!(!page.source_checks[1].captured);
}
#[tokio::test]
async fn notification_diagnostics_bounds_saved_reasons_without_returning_source_payloads() {
    use server_api::NotificationDiagnosticsQuery as Q;
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) SELECT sequence,workspace_id,'bounded-reason','suppress',?1,'attention' FROM notification_sources WHERE assignment_id='a' LIMIT 1",["長".repeat(2000)])?;
        Ok(())
    }).unwrap();
    let page = store
        .notification_diagnostics(
            "w",
            &Q {
                delivery_id: Some("bounded-reason".into()),
                ..Q::default()
            },
        )
        .unwrap();
    assert_eq!(page.items[0].reason.as_ref().unwrap().chars().count(), 512);
    assert!(!serde_json::to_string(&page).unwrap().contains("facts_json"));
}
#[tokio::test]
async fn notification_diagnostics_detects_unconnected_origin_and_retains_history_without_pending_work()
 {
    use server_api::{NotificationDiagnosticStage as S, NotificationDiagnosticsQuery as Q};
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let delivery = delivery_store_reserve_initial(&store, &tracking_store_source());
    store
        .clear_current_ticket_role_assignment(
            "w",
            "tracking",
            TicketAssignmentRole::Orchestrator,
            "a",
            "clear-a",
            "clear-a",
            "owner",
            "2",
            None,
        )
        .unwrap();
    let page = store
        .notification_diagnostics(
            "w",
            &Q {
                delivery_id: Some(delivery.delivery_id),
                ..Q::default()
            },
        )
        .unwrap();
    assert_eq!(page.items[0].stage, S::Historical);
    assert_eq!(page.items[0].recipient_current, Some(false));
    assert!(!page.items[0].retry_candidate);
    // Simulate a missing producer adapter; a diagnostic read must not repair or hide it.
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER notification_ticket_event")?;
            Ok(())
        })
        .unwrap();
    delivery_store_external_event(&store, 999, "comment", None);
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA query_only=ON")?;
            Ok(())
        })
        .unwrap();
    let query = Q {
        ticket: Some("tracking".into()),
        source_event_id: Some("ticket-event:999".into()),
        ..Q::default()
    };
    let page = store.notification_diagnostics("w", &query).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.source_checks[0].origin_present, Some(true));
    assert!(!page.source_checks[0].captured);
    let other = store.notification_diagnostics("other", &query).unwrap();
    assert_eq!(other.source_checks[0].origin_present, Some(false));
    assert!(!other.source_checks[0].captured);
}
#[tokio::test]
async fn notification_diagnostics_is_readonly_paginated_and_workspace_scoped() {
    use server_api::{NotificationDiagnosticStage as S, NotificationDiagnosticsQuery as Q};
    let (_dir, store) = delivery_tracking_fixture().await;
    delivery_tracking_assign(&store, "a", "assign-a");
    delivery_store_answer(&store, "answer-a");
    let before = store
        .with_conn(|conn| {
            Ok((
                conn.total_changes(),
                stored_fixture_rows(conn, "notification_decisions"),
            ))
        })
        .unwrap();
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA query_only=ON")?;
            Ok(())
        })
        .unwrap();
    let first = store
        .notification_diagnostics(
            "w",
            &Q {
                limit: Some(1),
                ..Q::default()
            },
        )
        .unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].stage, S::AwaitingRouting);
    let next = store
        .notification_diagnostics(
            "w",
            &Q {
                cursor: first.next_cursor.clone(),
                ..Q::default()
            },
        )
        .unwrap();
    assert!(
        next.items
            .iter()
            .all(|i| i.source_id != first.items[0].source_id)
    );
    assert!(!next.items.is_empty());
    assert!(
        store
            .notification_diagnostics(
                "other-workspace",
                &Q {
                    cursor: Some(first.items[0].source_id.clone()),
                    ..Q::default()
                }
            )
            .is_err()
    );
    assert!(
        store
            .notification_diagnostics(
                "w",
                &Q {
                    ticket: Some("missing".into()),
                    cursor: Some(first.items[0].source_id.clone()),
                    ..Q::default()
                }
            )
            .is_err()
    );
    for limit in [0, 101] {
        assert!(
            store
                .notification_diagnostics(
                    "w",
                    &Q {
                        limit: Some(limit),
                        ..Q::default()
                    }
                )
                .is_err()
        );
    }
    assert!(
        store
            .notification_diagnostics(
                "w",
                &Q {
                    cursor: Some("not-a-source".into()),
                    ..Q::default()
                }
            )
            .is_err()
    );
    assert_eq!(
        store
            .with_conn(|conn| Ok((
                conn.total_changes(),
                stored_fixture_rows(conn, "notification_decisions")
            )))
            .unwrap(),
        before
    );
    store
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA query_only=OFF")?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn notification_diagnostics_preserves_suppression_failure_and_acceptance_boundaries() {
    use server_api::{NotificationDiagnosticStage as S, NotificationDiagnosticsQuery as Q};
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    let pending = store
        .list_pending_ticket_assignment_deliveries("w", 100)
        .unwrap();
    assert_eq!(pending.len(), 1);
    let query = Q {
        assignment_id: Some("a".into()),
        source_event_id: Some(pending[0].input_event_id.clone()),
        ..Q::default()
    };
    let read = || {
        store
            .notification_diagnostics("w", &query)
            .unwrap()
            .items
            .remove(0)
    };
    assert_eq!(read().stage, S::Pending);
    let source = tracking_store_source();
    let delivery = delivery_store_reserve_initial(&store, &source);
    assert_eq!(read().stage, S::Reserved);
    store
        .prepare_ticket_assignment_delivery_content(
            "w",
            &delivery.delivery_id,
            "PRIVATE-NOTIFICATION-CONTENT",
            &source,
        )
        .unwrap();
    assert_eq!(read().stage, S::RequestPersisted);
    let key = NotificationDeliveryKey::AssignmentInput {
        assignment_id: delivery.assignment_id.clone(),
        input_event_id: delivery.input_event_id.clone(),
    };
    store
        .record_notification_delivery_failure("w", &key, NotificationFailureCode::RuntimeRejected)
        .unwrap();
    assert_eq!(read().stage, S::RuntimeRejected);
    store
        .record_notification_delivery_failure("w", &key, NotificationFailureCode::AcceptanceUnknown)
        .unwrap();
    assert_eq!(read().stage, S::AcceptanceUnknown);
    store
        .ack_ticket_assignment_delivery("w", &delivery.delivery_id, &source)
        .unwrap();
    let accepted = read();
    assert_eq!(accepted.stage, S::RuntimeAccepted);
    assert!(!accepted.retry_candidate && !accepted.model_processing_confirmed);
    assert_eq!(
        accepted.target_worker_id.as_deref(),
        Some(source.worker_id.as_str())
    );
    let serialized = serde_json::to_string(&accepted).unwrap();
    assert!(!serialized.contains("PRIVATE-NOTIFICATION-CONTENT"));
    assert!(!serialized.contains(&source.authentication_binding_id));
    let all = store.notification_diagnostics("w", &Q::default()).unwrap();
    assert!(
        all.items
            .iter()
            .any(|i| i.stage == S::Suppressed && i.reason.as_deref() == Some("no_recipient"))
    );
    store.with_conn_mut(|conn| {
        conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) SELECT workspace_id,ticket_id,'future-event',role,assignment_id,'unregistered-event',facts_json FROM notification_sources WHERE assignment_id='a' LIMIT 1",[])?;
        notification_routing::route_pending(conn,"w")
    }).unwrap();
    let unknown = store
        .notification_diagnostics(
            "w",
            &Q {
                source_event_id: Some("future-event".into()),
                ..Q::default()
            },
        )
        .unwrap();
    assert_eq!(unknown.items[0].stage, S::Deferred);
    assert_eq!(unknown.items[0].reason.as_deref(), Some("unknown_event"));
    assert!(!unknown.items[0].retry_candidate);
}

#[tokio::test]
async fn notification_source_metadata_resolves_hashed_and_legacy_identities_without_job_body() {
    use server_api::authorization::PermissionResource;
    let (_dir, store) = tracking_store_fixture().await;
    let (_, source) = job_notice_fixture(&store, "metadata-only", false);
    let job = store
        .get_backend_job("w", "metadata-only")
        .unwrap()
        .unwrap();
    let retired = store
        .reclaim_backend_job_input(
            "w",
            "metadata-only",
            &job.input_digest,
            crate::backend_job::JobInputRetirementReason::ExplicitErasure,
        )
        .unwrap();
    assert!(retired.load_executable_request().is_err());
    store.with_conn(|conn| {
        // Retired protected input cannot execute, but notification ownership
        // remains resolvable from source metadata without restoring that input.
        for event in ["legacy-arbitrary-source", "mr-event:legacy-job:e"] {
            conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) SELECT workspace_id,ticket_id,?2,role,assignment_id,kind,facts_json FROM notification_sources WHERE input_event_id=?1", params![source.delivery_id,event])?;
        }
        Ok(())
    }).unwrap();
    for event in [
        source.delivery_id.as_str(),
        "legacy-arbitrary-source",
        "mr-event:legacy-job:e",
    ] {
        assert_eq!(
            store.notification_source_resource("w", event).unwrap(),
            Some(PermissionResource::Job {
                workspace_id: "w".into(),
                job_id: "metadata-only".into()
            })
        );
        let other = store.notification_source_resource("other", event).unwrap();
        if event == "mr-event:legacy-job:e" {
            assert_eq!(
                other,
                Some(PermissionResource::MergeRequest {
                    workspace_id: "other".into(),
                    merge_request_id: "legacy-job".into()
                })
            );
        } else {
            assert!(other.is_none());
        }
    }
    assert!(
        store
            .notification_source_resource("w", "unknown")
            .unwrap()
            .is_none()
    );
    tracking_store_assign(&store, "metadata-assignment", "metadata-assign");
    store.with_conn(|conn| {
        // A collision across distinct persisted owners must not select the first.
        conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) SELECT workspace_id,ticket_id,?1,role,assignment_id,kind,facts_json FROM notification_sources WHERE assignment_id='metadata-assignment' LIMIT 1", [&source.delivery_id])?;
        Ok(())
    }).unwrap();
    assert!(
        store
            .notification_source_resource("w", &source.delivery_id)
            .unwrap()
            .is_none()
    );
}
