use llm_worker::WorkerResult;
use llm_worker::llm_client::types::{Item, RequestConfig};
use session_store::{
    FsStore, LogEntry, Store, TraceEntry, collect_state, new_segment_id, new_session_id,
};

fn nil_session_start(ts: u64, session_id: uuid::Uuid) -> LogEntry {
    LogEntry::SegmentStart {
        ts,
        session_id,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![],
        forked_from: None,
        compacted_from: None,
    }
}

#[test]
fn round_trip_write_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    let entries = vec![
        LogEntry::SegmentStart {
            ts: 1000,
            session_id: sid,
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        },
        LogEntry::UserInput {
            ts: 2000,
            segments: vec![protocol::Segment::text("Hello")],
        },
        LogEntry::AssistantItem {
            ts: 3000,
            item: Item::assistant_message("Hi there!").into(),
        },
        LogEntry::TurnEnd {
            ts: 3100,
            turn_count: 1,
        },
        LogEntry::RunCompleted {
            ts: 3200,
            interrupted: false,
            result: WorkerResult::Finished,
        },
    ];

    for entry in &entries {
        store.append(sid, segid, entry).unwrap();
    }

    let read_back = store.read_all(sid, segid).unwrap();
    assert_eq!(read_back.len(), entries.len());

    let state = collect_state(&read_back);
    assert_eq!(state.session_id, Some(sid));
    assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
    assert_eq!(state.config.max_tokens, Some(1024));
    assert_eq!(state.history.len(), 2);
    assert_eq!(state.turn_count, 1);
    assert!(!state.last_run_interrupted);
    assert_eq!(state.entries_count, entries.len());
}

#[test]
fn create_segment_writes_all_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    let entries = [LogEntry::SegmentStart {
        ts: 1000,
        session_id: sid,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![
            Item::user_message("seed").into(),
            Item::assistant_message("ok").into(),
        ],
        forked_from: None,
        compacted_from: None,
    }];

    store.create_segment(sid, segid, &entries).unwrap();
    let read_back = store.read_all(sid, segid).unwrap();
    assert_eq!(read_back.len(), 1);

    let state = collect_state(&read_back);
    assert_eq!(state.history.len(), 2);
    assert_eq!(state.session_id, Some(sid));
}

#[test]
fn list_sessions_and_segments() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();

    let sid_a = new_session_id();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let sid_b = new_session_id();

    let seg_a1 = new_segment_id();
    let seg_a2 = new_segment_id();
    let seg_b1 = new_segment_id();

    store.append(sid_a, seg_a1, &nil_session_start(1, sid_a)).unwrap();
    store.append(sid_a, seg_a2, &nil_session_start(2, sid_a)).unwrap();
    store.append(sid_b, seg_b1, &nil_session_start(3, sid_b)).unwrap();

    let sessions = store.list_sessions().unwrap();
    assert_eq!(sessions, vec![sid_b, sid_a]); // newest first

    let segs_a = store.list_segments(sid_a).unwrap();
    assert!(segs_a.contains(&seg_a1) && segs_a.contains(&seg_a2));
    assert_eq!(segs_a.len(), 2);

    let segs_b = store.list_segments(sid_b).unwrap();
    assert_eq!(segs_b, vec![seg_b1]);
}

#[test]
fn exists_returns_correct_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    assert!(!store.exists(sid, segid).unwrap());

    store.append(sid, segid, &nil_session_start(1000, sid)).unwrap();

    assert!(store.exists(sid, segid).unwrap());
}

#[test]
fn not_found_error_for_missing_segment() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    let result = store.read_all(sid, segid);
    assert!(result.is_err());
}

#[test]
fn trace_entries_in_separate_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    store.append(sid, segid, &nil_session_start(1000, sid)).unwrap();

    let trace = TraceEntry {
        ts: 1500,
        turn: 0,
        event: llm_worker::llm_client::event::Event::Ping(
            llm_worker::llm_client::event::PingEvent { timestamp: None },
        ),
    };
    store.append_trace(sid, segid, &trace).unwrap();

    // Log should have 1 entry, unaffected by trace
    let log = store.read_all(sid, segid).unwrap();
    assert_eq!(log.len(), 1);

    // Trace file should exist separately
    let trace_path = dir
        .path()
        .join(sid.to_string())
        .join(format!("{segid}.trace.jsonl"));
    assert!(trace_path.exists());
}

#[test]
fn read_entry_count_matches_append_tally() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    let entries = [
        LogEntry::SegmentStart {
            ts: 1000,
            session_id: sid,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        },
        LogEntry::UserInput {
            ts: 2000,
            segments: vec![protocol::Segment::text("Hello")],
        },
    ];

    for entry in &entries {
        store.append(sid, segid, entry).unwrap();
    }

    assert_eq!(store.read_entry_count(sid, segid).unwrap(), entries.len());
}

#[test]
fn lookup_session_of_finds_owning_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).unwrap();
    let sid = new_session_id();
    let segid = new_segment_id();

    assert_eq!(store.lookup_session_of(segid).unwrap(), None);

    store.append(sid, segid, &nil_session_start(1, sid)).unwrap();

    assert_eq!(store.lookup_session_of(segid).unwrap(), Some(sid));
}
