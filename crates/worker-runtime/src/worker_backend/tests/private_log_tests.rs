// Real public log projection and protocol bridge; channel fixtures replace only
// the Controller producer. Private entries must not terminate either receive path.
use super::*;

fn private_log_entries() -> Vec<LogEntry> {
    [
        LoggedItem::AssistantResponseBoundary {
            response_id: "private-response".into(),
        },
        LoggedItem::Reasoning {
            text: "private reasoning".into(),
            summary: vec![],
            encrypted_content: None,
            signature: None,
        },
    ]
    .into_iter()
    .map(|item| LogEntry::AnnotatedAssistantItem {
        ts: 1,
        entry: LoggedHistoryEntry {
            item,
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId::new(),
                origin: LoggedSessionHistoryOrigin::LegacyUnknown,
                derivation: None,
                input_metadata: None,
            },
        },
    })
    .collect()
}

#[tokio::test]
async fn protocol_bridge_private_logs_preserve_public_history_and_later_state() {
    let initial = protocol::WorkerStateSnapshot::initial();
    let latest = protocol::WorkerStateSnapshot {
        state_sequence: 1,
        last_finished_submission_request_id: Some("finished-request".into()),
        ..initial.clone()
    };
    let (streams, events, logs) = bridge_test_streams(4, initial.clone());
    for entry in private_log_entries() {
        logs.send(entry).unwrap();
    }
    let public = LogEntry::RunCancelled {
        ts: 2,
        entry_id: Some(LoggedSessionHistoryEntryId::new()),
    };
    logs.send(public.clone()).unwrap();
    let mut events = Some(events);
    let current = Arc::new(RwLock::new(initial));
    let mut published = vec![];
    let mut invalidations = vec![];
    tokio::time::timeout(
        Duration::from_secs(5),
        run_protocol_bridge(
            streams,
            current.clone(),
            Ok,
            || panic!("private entries must not require recovery"),
            |event| {
                match &event {
                    Event::SessionEntryCommitted { .. } => {
                        events
                            .as_ref()
                            .unwrap()
                            .send(Event::WorkerState {
                                snapshot: latest.clone(),
                            })
                            .unwrap();
                    }
                    Event::WorkerState { .. } => {
                        events.take();
                    }
                    _ => panic!("private entries must not be published"),
                }
                published.push(event);
                Ok(())
            },
            |reason| {
                invalidations.push(reason.to_string());
                Ok(())
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(published.len(), 2);
    assert_eq!(
        serde_json::to_value(&published[0]).unwrap(),
        serde_json::to_value(live_log_entry_event(public).unwrap()).unwrap(),
    );
    assert!(matches!(&published[1], Event::WorkerState { snapshot } if *snapshot == latest));
    assert_eq!(*current.read().unwrap(), latest);
    assert_eq!(invalidations, ["Worker protocol event stream closed"]);
}

#[tokio::test]
async fn protocol_bridge_resnapshot_skips_private_pending_logs_without_losing_public_facts() {
    let initial = protocol::WorkerStateSnapshot::initial();
    let (streams, events, logs) = bridge_test_streams(4, initial.clone());
    // Trigger recovery before adding logs so the pending-drain path is deterministic.
    events.send(Event::Shutdown).unwrap();
    let (replacement, replacement_events, _replacement_logs) =
        bridge_test_streams(4, initial.clone());
    let mut replacement = Some(replacement);
    let mut replacement_events = Some(replacement_events);
    let public = LogEntry::RunCancelled {
        ts: 2,
        entry_id: Some(LoggedSessionHistoryEntryId::new()),
    };
    let mut published = vec![];
    let mut invalidations = vec![];
    tokio::time::timeout(
        Duration::from_secs(5),
        run_protocol_bridge(
            streams,
            Arc::new(RwLock::new(initial)),
            |_| Err(()),
            || {
                for entry in private_log_entries() {
                    logs.send(entry).unwrap();
                }
                logs.send(public.clone()).unwrap();
                replacement.take().expect("one resnapshot")
            },
            |event| {
                if matches!(event, Event::Snapshot { .. }) {
                    replacement_events.take();
                }
                published.push(event);
                Ok(())
            },
            |reason| {
                invalidations.push(reason.to_string());
                Ok(())
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(published.len(), 2);
    assert_eq!(
        serde_json::to_value(&published[0]).unwrap(),
        serde_json::to_value(live_log_entry_event(public).unwrap()).unwrap(),
    );
    assert!(matches!(&published[1], Event::Snapshot { .. }));
    assert_eq!(invalidations, ["Worker protocol event stream closed"]);
}
