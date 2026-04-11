use llm_worker::llm_client::types::{Item, RequestConfig};
use session_store::{
    FsStore, LogEntry, Outcome, Store, TraceEntry, build_chain, collect_state, new_session_id,
};

#[tokio::test]
async fn round_trip_write_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    let id = new_session_id();

    let raw = vec![
        LogEntry::SessionStart {
            ts: 1000,
            system_prompt: Some("You are helpful.".into()),
            config: RequestConfig::default().with_max_tokens(1024),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        },
        LogEntry::UserInput {
            ts: 2000,
            item: Item::user_message("Hello"),
        },
        LogEntry::AssistantItems {
            ts: 3000,
            items: vec![Item::assistant_message("Hi there!")],
        },
        LogEntry::TurnEnd {
            ts: 3100,
            turn_count: 1,
        },
        LogEntry::RunOutcome {
            ts: 3200,
            outcome: Outcome::Finished,
            interrupted: false,
        },
    ];
    let entries = build_chain(&raw);

    // Write entries one by one
    for entry in &entries {
        store.append(id, entry).await.unwrap();
    }

    // Read back
    let read_back = store.read_all(id).await.unwrap();
    assert_eq!(read_back.len(), entries.len());

    // Verify hashes survived round-trip
    for (orig, read) in entries.iter().zip(read_back.iter()) {
        assert_eq!(orig.hash, read.hash);
        assert_eq!(orig.prev_hash, read.prev_hash);
    }

    // Replay and verify state
    let state = collect_state(&read_back);
    assert_eq!(state.system_prompt.as_deref(), Some("You are helpful."));
    assert_eq!(state.config.max_tokens, Some(1024));
    assert_eq!(state.history.len(), 2);
    assert_eq!(state.turn_count, 1);
    assert!(!state.last_run_interrupted);
    assert!(state.head_hash.is_some());
}

#[tokio::test]
async fn create_session_writes_all_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    let id = new_session_id();

    let entries = build_chain(&[LogEntry::SessionStart {
        ts: 1000,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![Item::user_message("seed"), Item::assistant_message("ok")],
        forked_from: None,
        compacted_from: None,
    }]);

    store.create_session(id, &entries).await.unwrap();
    let read_back = store.read_all(id).await.unwrap();
    assert_eq!(read_back.len(), 1);

    let state = collect_state(&read_back);
    assert_eq!(state.history.len(), 2);
}

#[tokio::test]
async fn list_sessions_returns_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();

    let id1 = new_session_id();
    // Small delay to ensure different UUID v7 timestamps
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let id2 = new_session_id();

    let entries1 = build_chain(&[LogEntry::SessionStart {
        ts: 1000,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![],
        forked_from: None,
        compacted_from: None,
    }]);
    let entries2 = build_chain(&[LogEntry::SessionStart {
        ts: 1001,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![],
        forked_from: None,
        compacted_from: None,
    }]);

    store.append(id1, &entries1[0]).await.unwrap();
    store.append(id2, &entries2[0]).await.unwrap();

    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0], id2); // newest first
    assert_eq!(sessions[1], id1);
}

#[tokio::test]
async fn exists_returns_correct_state() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    let id = new_session_id();

    assert!(!store.exists(id).await.unwrap());

    let entries = build_chain(&[LogEntry::SessionStart {
        ts: 1000,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![],
        forked_from: None,
        compacted_from: None,
    }]);
    store.append(id, &entries[0]).await.unwrap();

    assert!(store.exists(id).await.unwrap());
}

#[tokio::test]
async fn not_found_error_for_missing_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    let id = new_session_id();

    let result = store.read_all(id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn trace_entries_in_separate_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    let id = new_session_id();

    // Write a log entry
    let entries = build_chain(&[LogEntry::SessionStart {
        ts: 1000,
        system_prompt: None,
        config: RequestConfig::default(),
        history: vec![],
        forked_from: None,
        compacted_from: None,
    }]);
    store.append(id, &entries[0]).await.unwrap();

    // Write a trace entry
    let trace = TraceEntry {
        ts: 1500,
        turn: 0,
        event: llm_worker::llm_client::event::Event::Ping(
            llm_worker::llm_client::event::PingEvent { timestamp: None },
        ),
    };
    store.append_trace(id, &trace).await.unwrap();

    // Log should have 1 entry, unaffected by trace
    let log = store.read_all(id).await.unwrap();
    assert_eq!(log.len(), 1);

    // Trace file should exist separately
    let trace_path = dir.path().join(format!("{id}.trace.jsonl"));
    assert!(trace_path.exists());
}

#[tokio::test]
async fn read_head_hash_returns_last_entry_hash() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::new(dir.path()).await.unwrap();
    let id = new_session_id();

    let entries = build_chain(&[
        LogEntry::SessionStart {
            ts: 1000,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        },
        LogEntry::UserInput {
            ts: 2000,
            item: Item::user_message("Hello"),
        },
    ]);

    for entry in &entries {
        store.append(id, entry).await.unwrap();
    }

    let head = store.read_head_hash(id).await.unwrap();
    assert_eq!(head.as_ref(), Some(&entries[1].hash));
}
