// Production Store and SQLite. No Runtime/model side effects are required for diagnostics.
#[tokio::test]
async fn notification_diagnostics_bounds_saved_reasons_without_returning_source_payloads() {
    use server_api::NotificationDiagnosticsQuery as Q;
    let (_dir,store)=tracking_store_fixture().await;
    tracking_store_assign(&store,"a","assign-a");
    store.with_conn(|conn| {
        conn.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) SELECT sequence,workspace_id,'bounded-reason','suppress',?1,'attention' FROM notification_sources WHERE assignment_id='a' LIMIT 1",["長".repeat(2000)])?;
        Ok(())
    }).unwrap();
    let page=store.notification_diagnostics("w",&Q{delivery_id:Some("bounded-reason".into()),..Q::default()}).unwrap();
    assert_eq!(page.items[0].reason.as_ref().unwrap().chars().count(),512);
    assert!(!serde_json::to_string(&page).unwrap().contains("facts_json"));
}
#[tokio::test]
async fn notification_diagnostics_detects_unconnected_origin_and_retains_history_without_pending_work() {
    use server_api::{NotificationDiagnosticsQuery as Q,NotificationDiagnosticStage as S};
    let (_dir,store)=tracking_store_fixture().await;
    tracking_store_assign(&store,"a","assign-a");
    let delivery=delivery_store_reserve_initial(&store,&tracking_store_source());
    store.clear_current_ticket_role_assignment("w","tracking",TicketAssignmentRole::Orchestrator,"a","clear-a","clear-a","owner","2",None).unwrap();
    let page=store.notification_diagnostics("w",&Q{delivery_id:Some(delivery.delivery_id),..Q::default()}).unwrap();
    assert_eq!(page.items[0].stage,S::Historical);
    assert_eq!(page.items[0].recipient_current,Some(false));
    assert!(!page.items[0].retry_candidate);
    // Simulate a missing producer adapter; a diagnostic read must not repair or hide it.
    store.with_conn(|conn|{conn.execute_batch("DROP TRIGGER notification_ticket_event")?;Ok(())}).unwrap();
    delivery_store_external_event(&store,999,"comment",None);
    store.with_conn(|conn|{conn.execute_batch("PRAGMA query_only=ON")?;Ok(())}).unwrap();
    let query=Q{ticket:Some("tracking".into()),source_event_id:Some("ticket-event:999".into()),..Q::default()};
    let page=store.notification_diagnostics("w",&query).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.source_checks[0].origin_present,Some(true));
    assert!(!page.source_checks[0].captured);
    let other=store.notification_diagnostics("other",&query).unwrap();
    assert_eq!(other.source_checks[0].origin_present,Some(false));
    assert!(!other.source_checks[0].captured);
}
#[tokio::test]
async fn notification_diagnostics_is_readonly_paginated_and_workspace_scoped() {
    use server_api::{NotificationDiagnosticsQuery as Q, NotificationDiagnosticStage as S};
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store, "a", "assign-a");
    delivery_store_answer(&store, "answer-a");
    let before = store.with_conn(|conn| Ok((conn.total_changes(),stored_fixture_rows(conn,"notification_decisions")))).unwrap();
    store.with_conn(|conn| { conn.execute_batch("PRAGMA query_only=ON")?; Ok(()) }).unwrap();
    let first = store.notification_diagnostics("w", &Q{limit:Some(1), ..Q::default()}).unwrap();
    assert_eq!(first.items.len(),1);
    assert_eq!(first.items[0].stage,S::AwaitingRouting);
    let next = store.notification_diagnostics("w", &Q{cursor:first.next_cursor.clone(), ..Q::default()}).unwrap();
    assert!(next.items.iter().all(|i| i.source_id != first.items[0].source_id));
    assert!(!next.items.is_empty());
    assert!(store.notification_diagnostics("other-workspace", &Q{cursor:Some(first.items[0].source_id.clone()), ..Q::default()}).is_err());
    assert!(store.notification_diagnostics("w", &Q{ticket:Some("missing".into()), cursor:Some(first.items[0].source_id.clone()), ..Q::default()}).is_err());
    for limit in [0,101] { assert!(store.notification_diagnostics("w", &Q{limit:Some(limit), ..Q::default()}).is_err()); }
    assert!(store.notification_diagnostics("w", &Q{cursor:Some("not-a-source".into()), ..Q::default()}).is_err());
    assert_eq!(store.with_conn(|conn| Ok((conn.total_changes(),stored_fixture_rows(conn,"notification_decisions")))).unwrap(),before);
    store.with_conn(|conn| { conn.execute_batch("PRAGMA query_only=OFF")?; Ok(()) }).unwrap();
}

#[tokio::test]
async fn notification_diagnostics_preserves_suppression_failure_and_acceptance_boundaries() {
    use server_api::{NotificationDiagnosticsQuery as Q, NotificationDiagnosticStage as S};
    let (_dir, store) = tracking_store_fixture().await;
    tracking_store_assign(&store,"a","assign-a");
    let pending = store.list_pending_ticket_assignment_deliveries("w",100).unwrap();
    assert_eq!(pending.len(),1);
    let query = Q{assignment_id:Some("a".into()), source_event_id:Some(pending[0].input_event_id.clone()), ..Q::default()};
    let read = || store.notification_diagnostics("w", &query).unwrap().items.remove(0);
    assert_eq!(read().stage,S::Pending);
    let source = tracking_store_source();
    let delivery = delivery_store_reserve_initial(&store,&source);
    assert_eq!(read().stage,S::Reserved);
    store.prepare_ticket_assignment_delivery_content("w",&delivery.delivery_id,"PRIVATE-NOTIFICATION-CONTENT",&source).unwrap();
    assert_eq!(read().stage,S::RequestPersisted);
    let key = NotificationDeliveryKey::AssignmentInput{assignment_id:delivery.assignment_id.clone(),input_event_id:delivery.input_event_id.clone()};
    store.record_notification_delivery_failure("w",&key,NotificationFailureCode::RuntimeRejected).unwrap();
    assert_eq!(read().stage,S::RuntimeRejected);
    store.record_notification_delivery_failure("w",&key,NotificationFailureCode::AcceptanceUnknown).unwrap();
    assert_eq!(read().stage,S::AcceptanceUnknown);
    store.ack_ticket_assignment_delivery("w",&delivery.delivery_id,&source).unwrap();
    let accepted = read();
    assert_eq!(accepted.stage,S::RuntimeAccepted);
    assert!(!accepted.retry_candidate && !accepted.model_processing_confirmed);
    assert_eq!(accepted.target_worker_id.as_deref(),Some(source.worker_id.as_str()));
    let serialized = serde_json::to_string(&accepted).unwrap();
    assert!(!serialized.contains("PRIVATE-NOTIFICATION-CONTENT"));
    assert!(!serialized.contains(&source.authentication_binding_id));
    let all = store.notification_diagnostics("w",&Q::default()).unwrap();
    assert!(all.items.iter().any(|i| i.stage==S::Suppressed && i.reason.as_deref()==Some("no_recipient")));
    store.with_conn_mut(|conn| {
        conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) SELECT workspace_id,ticket_id,'future-event',role,assignment_id,'unregistered-event',facts_json FROM notification_sources WHERE assignment_id='a' LIMIT 1",[])?;
        notification_routing::route_pending(conn,"w")
    }).unwrap();
    let unknown = store.notification_diagnostics("w",&Q{source_event_id:Some("future-event".into()), ..Q::default()}).unwrap();
    assert_eq!(unknown.items[0].stage,S::Deferred);
    assert_eq!(unknown.items[0].reason.as_deref(),Some("unknown_event"));
    assert!(!unknown.items[0].retry_candidate);
}
