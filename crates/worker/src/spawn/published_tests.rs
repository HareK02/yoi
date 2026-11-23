//! Component tests of the published-stream projection. Only the source/history
//! resolver is a session fixture; state, nested delivery and recovery use the
//! production reducer. No transport, Runtime, persistence or UI claim is made.
use super::*;
use crate::internal_worker::{InternalWorkerVisibility, test_internal_worker_session};
use protocol::{
    Greeting, InternalWorkerKind, InternalWorkerRef, WorkerStateSnapshot, WorkerStatus,
};

fn worker(id: &str, sequence: u64, status: WorkerStatus) -> InternalWorkerSnapshot {
    let mut state: WorkerStateSnapshot = status.into();
    state.state_sequence = sequence;
    InternalWorkerSnapshot {
        worker: InternalWorkerRef {
            session_id: id.into(),
            name: "same-display-name".into(),
            parent_session_id: None,
            kind: InternalWorkerKind::SubWorker,
        },
        session: protocol::SessionSnapshot {
            pending_submissions: Default::default(),
            entries: vec![],
        },
        greeting: Some(Greeting {
            worker_name: id.into(),
            cwd: "/fixture".into(),
            provider: "fixture".into(),
            model: "none".into(),
            reasoning: None,
            scope_summary: String::new(),
            tools: vec![],
            context_window: 0,
            context_tokens: 0,
            context_usage: None,
        }),
        status,
        state: Some(state),
        error: None,
        in_flight: Default::default(),
        internal_workers: vec![],
    }
}

fn snapshot_event(snapshot: &InternalWorkerSnapshot) -> Event {
    Event::Snapshot {
        session: snapshot.session.clone(),
        greeting: snapshot.greeting.clone().unwrap(),
        state: snapshot.state.clone().unwrap(),
        in_flight: snapshot.in_flight.clone(),
        internal_workers: snapshot.internal_workers.clone(),
    }
}

fn nested(child: &InternalWorkerSnapshot, event: Event) -> Event {
    Event::InternalWorker {
        worker: child.worker.clone(),
        event: Box::new(event),
    }
}

fn finished(sequence: u64, id: &str) -> WorkerStateSnapshot {
    let mut state: WorkerStateSnapshot = WorkerStatus::Idle.into();
    state.state_sequence = sequence;
    state.last_finished_submission_request_id = Some(id.into());
    state
}

fn assert_finished(snapshot: &InternalWorkerSnapshot, sequence: u64, id: &str) {
    assert_eq!(snapshot.status, WorkerStatus::Idle);
    assert_eq!(snapshot.state.as_ref(), Some(&finished(sequence, id)));
    assert!(snapshot.in_flight.is_empty());
}

#[tokio::test]
async fn parent_snapshot_preserves_independent_child_and_grandchild_completion() {
    let (source, _tx) = test_internal_worker_session(InternalWorkerVisibility::ParentClient);
    let grandchild = worker("grandchild", 30, WorkerStatus::Running);
    let mut child = worker("child", 3, WorkerStatus::Running);
    child.internal_workers.push(grandchild.clone());
    let mut parent = worker("parent", 10, WorkerStatus::Idle);
    parent.internal_workers.push(child.clone());
    let mut published = PublishedSnapshot::new(parent.clone());
    apply(
        &mut published,
        &nested(
            &child,
            Event::WorkerState {
                snapshot: finished(4, "child-run"),
            },
        ),
        &source,
    );
    apply(
        &mut published,
        &nested(
            &child,
            nested(
                &grandchild,
                Event::WorkerState {
                    snapshot: finished(31, "grandchild-run"),
                },
            ),
        ),
        &source,
    );
    // Equal, newer and older parent sequences are unrelated to either child.
    for sequence in [10, 11, 9] {
        parent.state.as_mut().unwrap().state_sequence = sequence;
        apply(&mut published, &snapshot_event(&parent), &source);
        let child = &published.snapshot.internal_workers[0];
        assert_finished(child, 4, "child-run");
        assert_finished(&child.internal_workers[0], 31, "grandchild-run");
    }
    // A stale intermediate child must not suppress a newer grandchild either.
    parent.internal_workers[0].internal_workers[0].state =
        Some(finished(32, "next-grandchild-run"));
    parent.internal_workers[0].internal_workers[0].status = WorkerStatus::Idle;
    apply(&mut published, &snapshot_event(&parent), &source);
    assert_finished(&published.snapshot.internal_workers[0], 4, "child-run");
    assert_finished(
        &published.snapshot.internal_workers[0].internal_workers[0],
        32,
        "next-grandchild-run",
    );
}

#[tokio::test]
async fn unavailable_child_retains_high_water_until_fresh_paired_snapshot() {
    let (source, _tx) = test_internal_worker_session(InternalWorkerVisibility::ParentClient);
    for closure in ["none", "unavailable", "shutdown"] {
        let mut grandchild = worker("grandchild", 41, WorkerStatus::Idle);
        grandchild.state = Some(finished(41, "grandchild-run"));
        let mut child = worker("child", 4, WorkerStatus::Idle);
        child.state = Some(finished(4, "child-run"));
        child.internal_workers.push(grandchild.clone());
        let mut parent = worker("parent", 1, WorkerStatus::Idle);
        parent.internal_workers.push(child.clone());
        let mut published = PublishedSnapshot::new(parent.clone());
        let close = match closure {
            "none" => {
                let mut unavailable = parent.clone();
                unavailable.internal_workers[0].state = None;
                snapshot_event(&unavailable)
            }
            "unavailable" => Event::InternalWorkerObservationUnavailable {
                worker: child.worker.clone(),
            },
            _ => nested(&child, Event::Shutdown),
        };
        apply(&mut published, &close, &source);
        assert!(published.snapshot.internal_workers[0].state.is_none());
        assert!(
            published.snapshot.internal_workers[0].internal_workers[0]
                .state
                .is_none()
        );
        if closure == "shutdown" {
            assert_eq!(
                published.snapshot.internal_workers[0].status,
                WorkerStatus::Stopped
            );
        }
        // Neither queued live states nor queued child Snapshots can reopen it.
        let mut queued = child.clone();
        queued.state = Some(finished(100, "queued"));
        for event in [
            Event::WorkerState {
                snapshot: finished(100, "queued"),
            },
            snapshot_event(&queued),
        ] {
            apply(&mut published, &nested(&child, event), &source);
            assert!(published.snapshot.internal_workers[0].state.is_none());
        }
        // A paired snapshot below the previous child's sequence is still stale.
        let mut stale = parent.clone();
        stale.internal_workers[0].state = Some(finished(3, "older-child"));
        stale.internal_workers[0].internal_workers[0].state =
            Some(finished(40, "older-grandchild"));
        apply(&mut published, &snapshot_event(&stale), &source);
        assert!(published.snapshot.internal_workers[0].state.is_none());
        assert!(
            published.snapshot.internal_workers[0].internal_workers[0]
                .state
                .is_none()
        );
        // Recovery is independent at every depth, including equal-sequence pairs.
        stale.internal_workers[0].state = child.state.clone();
        apply(&mut published, &snapshot_event(&stale), &source);
        assert_finished(&published.snapshot.internal_workers[0], 4, "child-run");
        assert!(
            published.snapshot.internal_workers[0].internal_workers[0]
                .state
                .is_none()
        );
        apply(&mut published, &snapshot_event(&parent), &source);
        assert_finished(
            &published.snapshot.internal_workers[0].internal_workers[0],
            41,
            "grandchild-run",
        );
    }
}

#[tokio::test]
async fn removal_and_session_replacement_do_not_reuse_old_child_observations() {
    let (source, _tx) = test_internal_worker_session(InternalWorkerVisibility::ParentClient);
    let child = worker("old-session", 40, WorkerStatus::Running);
    let mut parent = worker("parent", 1, WorkerStatus::Idle);
    parent.internal_workers.push(child.clone());
    let mut published = PublishedSnapshot::new(parent.clone());
    apply(
        &mut published,
        &Event::InternalWorkerRemoved {
            worker: child.worker.clone(),
        },
        &source,
    );
    apply(
        &mut published,
        &nested(&child, snapshot_event(&child)),
        &source,
    );
    assert!(published.snapshot.internal_workers.is_empty());
    let replacement = worker("replacement-session", 1, WorkerStatus::Idle);
    parent.internal_workers = vec![replacement.clone()];
    apply(&mut published, &snapshot_event(&parent), &source);
    assert_eq!(
        published.snapshot.internal_workers[0].worker.session_id,
        "replacement-session"
    );
    assert_eq!(
        published.snapshot.internal_workers[0].state,
        replacement.state
    );
    apply(
        &mut published,
        &Event::InternalWorkerObservationUnavailable {
            worker: child.worker.clone(),
        },
        &source,
    );
    apply(
        &mut published,
        &Event::InternalWorkerRemoved {
            worker: child.worker.clone(),
        },
        &source,
    );
    assert_eq!(
        published.snapshot.internal_workers[0].state,
        replacement.state
    );
    // Authoritative membership omission removes the session as well.
    parent.internal_workers.clear();
    apply(&mut published, &snapshot_event(&parent), &source);
    assert!(published.snapshot.internal_workers.is_empty());
    // A fresh paired membership can legitimately include a previously removed ID.
    parent.internal_workers.push(child.clone());
    apply(&mut published, &snapshot_event(&parent), &source);
    assert_eq!(published.snapshot.internal_workers[0].state, child.state);
}

#[tokio::test]
async fn root_invalidation_blocks_queued_snapshot_and_nested_state_recovery() {
    let (source, _tx) = test_internal_worker_session(InternalWorkerVisibility::ParentClient);
    let child = worker("child", 4, WorkerStatus::Idle);
    let mut parent = worker("parent", 1, WorkerStatus::Idle);
    parent.internal_workers.push(child.clone());
    let mut published = PublishedSnapshot::new(parent.clone());
    apply(
        &mut published,
        &Event::Error {
            code: protocol::ErrorCode::Internal,
            message: "diagnostic".into(),
        },
        &source,
    );
    assert!(published.snapshot.state.is_some());
    invalidate(&mut published);
    for event in [
        snapshot_event(&parent),
        nested(
            &child,
            Event::WorkerState {
                snapshot: finished(5, "queued"),
            },
        ),
    ] {
        apply(&mut published, &event, &source);
        assert!(published.snapshot.state.is_none());
        assert!(published.snapshot.internal_workers[0].state.is_none());
    }
}
