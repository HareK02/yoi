//! Real SQLite component tests through FeatureStorage and the chat APIs. Host authentication,
//! operation authorization and Backend singleton ownership are outside this suite; policy IDs
//! supplied here model the Host's observed existing policy, not a new authority source.
use super::*;
use crate::tests::{open_store, role};
use feature_storage::{FeatureRegistration, FeatureStorage};
use std::sync::{Arc, Barrier};

fn create(
    store: &SubjektivStore,
    account: &str,
    subject: &str,
    operation: &str,
) -> ChatConversation {
    store
        .chat_create_conversation(
            account,
            &ChatCreateConversationRequest {
                operation_id: operation.into(),
                subject_id: subject.into(),
            },
        )
        .unwrap()
}
fn user(operation: &str, text: &str) -> ChatUserMessageRequest {
    ChatUserMessageRequest {
        operation_id: operation.into(),
        text: text.into(),
        reply_to: None,
        work_ref: None,
    }
}
fn send(
    conversation: &ChatConversation,
    operation: &str,
    last_user_message_id: Option<&str>,
) -> ChatSendMessageRequest {
    ChatSendMessageRequest {
        conversation_id: conversation.id.clone(),
        operation_id: operation.into(),
        text: "public answer".into(),
        reply_to: None,
        work_ref: None,
        expected_last_user_message_id: last_user_message_id.map(str::to_owned),
    }
}
fn page(store: &SubjektivStore, conversation: &ChatConversation) -> ChatMessagePage {
    store
        .chat_messages_for_account(
            &conversation.participant_account_id,
            &conversation.id,
            &ChatMessagePageQuery::default(),
        )
        .unwrap()
}
fn sql(store: &SubjektivStore, sql: &str) {
    store
        .database
        .with_connection(|c| {
            c.execute_batch(sql)?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn membership_and_workspace_isolation_cover_reads_writes_and_replay() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let a = store.create_subject(role()).unwrap().id;
    let b = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &a, "create");
    let input = user("message", "private user message");
    store
        .chat_append_user_message("alice", &c.id, &input)
        .unwrap();
    for result in [
        store.chat_conversation_for_account("bob", &c.id),
        store.chat_conversation_for_subject(&b, &c.id),
    ] {
        assert!(matches!(result, Err(ChatError::ConversationNotFound)));
    }
    assert!(matches!(
        store.chat_messages_for_account("bob", &c.id, &ChatMessagePageQuery::default()),
        Err(ChatError::ConversationNotFound)
    ));
    assert!(matches!(
        store.chat_messages_for_subject(&b, &c.id, &ChatMessagePageQuery::default()),
        Err(ChatError::ConversationNotFound)
    ));
    assert!(matches!(
        store.chat_append_user_message("bob", &c.id, &input),
        Err(ChatError::ConversationNotFound)
    ));
    assert!(matches!(
        store.chat_send_message(&b, &send(&c, "message", None)),
        Err(ChatError::ConversationNotFound)
    ));
    let (_other_manager, _other_scope, other) = open_store(temp.path(), "workspace-b");
    assert!(matches!(
        other.chat_conversation_for_account("alice", &c.id),
        Err(ChatError::ConversationNotFound)
    ));
    assert!(matches!(
        other.chat_append_user_message("alice", &c.id, &input),
        Err(ChatError::ConversationNotFound)
    ));
    assert!(
        other
            .chat_create_conversation(
                "alice",
                &ChatCreateConversationRequest {
                    operation_id: "create".into(),
                    subject_id: a
                }
            )
            .is_err()
    );
    assert_eq!(page(&store, &c).messages.len(), 1);
}

#[test]
fn ack_loss_reopens_and_replays_original_results_after_new_input() {
    let temp = tempfile::tempdir().unwrap();
    let (c, first, answer, request) = {
        let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
        let subject = store.create_subject(role()).unwrap().id;
        let c = create(&store, "alice", &subject, "create");
        let first = store
            .chat_append_user_message("alice", &c.id, &user("same", "first"))
            .unwrap();
        let mut request = send(&c, "same", Some(&first.id)); // Sender namespaces are independent.
        request.reply_to = Some(first.id.clone());
        request.work_ref = Some("T-766".into());
        let answer = store.chat_send_message(&subject, &request).unwrap();
        store
            .chat_append_user_message("alice", &c.id, &user("next", "later input"))
            .unwrap();
        (c, first, answer, request)
    }; // Drop every connection: simulate committed storage with a lost acknowledgement.
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    assert_eq!(create(&store, "alice", &c.subject_id, "create"), c);
    assert_eq!(
        store
            .chat_append_user_message("alice", &c.id, &user("same", "first"))
            .unwrap(),
        first
    );
    assert_eq!(
        store.chat_send_message(&c.subject_id, &request).unwrap(),
        answer
    );
    let p = page(&store, &c);
    let last = &p.messages[2];
    assert_eq!(
        p.conversation.last_user_message_id.as_deref(),
        Some(last.id.as_str())
    );
    assert_eq!(
        first.last_user_message_id.as_deref(),
        Some(first.id.as_str())
    );
    assert_eq!(
        answer.last_user_message_id.as_deref(),
        Some(first.id.as_str())
    );
    assert_eq!(last.last_user_message_id.as_deref(), Some(last.id.as_str()));
    assert_eq!(
        p.messages.iter().map(|m| m.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(!p.has_more);
    assert_eq!(
        store
            .chat_messages_for_subject(&c.subject_id, &c.id, &ChatMessagePageQuery::default())
            .unwrap(),
        p
    );
}

#[test]
fn operation_reuse_checks_each_payload_field_and_pinned_observed_user_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let other_subject = store.create_subject(role()).unwrap().id;
    assert!(matches!(
        store.chat_create_conversation(
            "alice",
            &ChatCreateConversationRequest {
                operation_id: "create".into(),
                subject_id: other_subject
            }
        ),
        Err(ChatError::OperationConflict(_))
    ));
    // Same creation operation in another account namespace is a different operation.
    assert_ne!(create(&store, "bob", &subject, "create").id, c.id);
    let input = user("input", "first");
    let first = store
        .chat_append_user_message("alice", &c.id, &input)
        .unwrap();
    let mut variants = vec![input.clone(); 3];
    variants[0].text = "changed".into();
    variants[1].reply_to = Some(first.id.clone());
    variants[2].work_ref = Some("work".into());
    for variant in variants {
        assert!(matches!(
            store.chat_append_user_message("alice", &c.id, &variant),
            Err(ChatError::OperationConflict(_))
        ));
    }
    let request = send(&c, "answer", Some(&first.id));
    store.chat_send_message(&subject, &request).unwrap();
    let mut variants = vec![request.clone(); 4];
    variants[0].text = "changed".into();
    variants[1].reply_to = Some(first.id.clone());
    variants[2].work_ref = Some("work".into());
    variants[3].expected_last_user_message_id = None;
    for variant in variants {
        assert!(matches!(
            store.chat_send_message(&subject, &variant),
            Err(ChatError::OperationConflict(_))
        ));
    }
    assert_eq!(page(&store, &c).messages.len(), 2);
    assert_eq!(
        page(&store, &c)
            .conversation
            .last_user_message_id
            .as_deref(),
        Some(first.id.as_str())
    );
}

#[test]
fn reply_edges_cannot_cross_conversations_and_operations_are_conversation_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create-a");
    let d = create(&store, "alice", &subject, "create-b");
    let first = store
        .chat_append_user_message("alice", &c.id, &user("same", "a"))
        .unwrap();
    let mut bad = user("bad", "not a valid reply");
    bad.reply_to = Some(first.id.clone());
    assert!(matches!(
        store.chat_append_user_message("alice", &d.id, &bad),
        Err(ChatError::InvalidInput(_))
    ));
    let mut bad_send = send(&d, "bad", None);
    bad_send.reply_to = Some(first.id.clone());
    assert!(matches!(
        store.chat_send_message(&subject, &bad_send),
        Err(ChatError::InvalidInput(_))
    ));
    bad.reply_to = Some("missing".into());
    assert!(
        store
            .chat_append_user_message("alice", &c.id, &bad)
            .is_err()
    );
    assert!(page(&store, &d).messages.is_empty());
    assert_eq!(page(&store, &d).conversation.last_user_message_id, None);
    let other_input = store
        .chat_append_user_message("alice", &d.id, &user("same", "b"))
        .unwrap();
    // The composite FK rejects bypassing the repository too, not merely the ID lookup.
    let result = store.database.transaction(|tx| {
        tx.execute("INSERT INTO chat_messages (id,conversation_id,sequence,sender_kind,sender_id,text,reply_to,operation_id,last_user_message_id) VALUES ('bad',?1,2,'subject',?2,'bad',?3,'direct',?4)",params![d.id,subject,first.id,other_input.id])?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(page(&store, &d).messages.len(), 1);
}

#[test]
fn page_and_utf8_text_bounds_are_enforced_without_silent_clamping() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let exactly = "é".repeat(CHAT_MAX_TEXT_BYTES / 2);
    let first = store
        .chat_append_user_message("alice", &c.id, &user("limit", &exactly))
        .unwrap();
    for text in [String::new(), " \n".into(), format!("{exactly}x")] {
        assert!(matches!(
            store.chat_append_user_message("alice", &c.id, &user("bad", &text)),
            Err(ChatError::InvalidInput(_))
        ));
        let mut request = send(&c, "bad", Some(&first.id));
        request.text = text;
        assert!(matches!(
            store.chat_send_message(&subject, &request),
            Err(ChatError::InvalidInput(_))
        ));
    }
    for n in 2..=102 {
        store
            .chat_send_message(&subject, &send(&c, &format!("answer-{n}"), Some(&first.id)))
            .unwrap();
    }
    let query = |after, limit| ChatMessagePageQuery {
        after_sequence: after,
        limit,
    };
    for limit in [0, 101, u32::MAX] {
        assert!(matches!(
            store.chat_messages_for_account("alice", &c.id, &query(None, Some(limit))),
            Err(ChatError::InvalidInput(_))
        ));
    }
    let first = store
        .chat_messages_for_account("alice", &c.id, &query(None, Some(100)))
        .unwrap();
    assert_eq!(first.messages.len(), 100);
    assert!(first.has_more);
    let tail = store
        .chat_messages_for_account("alice", &c.id, &query(Some(100), Some(100)))
        .unwrap();
    assert_eq!(
        tail.messages.iter().map(|m| m.sequence).collect::<Vec<_>>(),
        vec![101, 102]
    );
    assert!(!tail.has_more);
    assert_eq!(
        page(&store, &c).messages.len(),
        CHAT_DEFAULT_PAGE_LIMIT as usize
    );
    assert!(matches!(
        store.chat_messages_for_account("alice", &c.id, &query(Some(u64::MAX), None)),
        Err(ChatError::InvalidInput(_))
    ));
}

#[test]
fn observed_user_identity_fence_and_append_share_one_sqlite_transaction_across_connections() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let (_second_manager, _second_scope, second) = open_store(temp.path(), "workspace-a");
    let barrier = Arc::new(Barrier::new(2));
    let (input, output) = std::thread::scope(|s| {
        let barrier = &barrier;
        let c = &c;
        let subject = &subject;
        let first = s.spawn(move || {
            barrier.wait();
            store.chat_append_user_message("alice", &c.id, &user("input", "new input"))
        });
        let other = s.spawn(move || {
            barrier.wait();
            second.chat_send_message(subject, &send(c, "answer", None))
        });
        (first.join().unwrap().unwrap(), other.join().unwrap())
    });
    // A subject send that won the SQLite write lock must precede user input. If input won,
    // no stale subject message can commit. Both connections must observe the same ordering.
    match output {
        Ok(answer) => {
            assert_eq!(answer.sequence, 1);
            assert_eq!(answer.last_user_message_id, None);
            assert_eq!(input.sequence, 2);
        }
        Err(ChatError::UserInputChanged {
            expected: None,
            actual,
        }) => {
            assert_eq!(actual.as_deref(), Some(input.id.as_str()));
            assert_eq!(input.sequence, 1);
        }
        other => panic!("unexpected race outcome: {other:?}"),
    }
    let (_manager, _scope, reopened) = open_store(temp.path(), "workspace-a");
    assert!(matches!(
        reopened.chat_send_message(&subject, &send(&c, "stale", None)),
        Err(ChatError::UserInputChanged {
            expected: None,
            actual: Some(ref actual)
        }) if actual == &input.id
    ));
    assert_eq!(
        page(&reopened, &c)
            .conversation
            .last_user_message_id
            .as_deref(),
        Some(input.id.as_str())
    );
    reopened
        .chat_send_message(&subject, &send(&c, "fresh", Some(&input.id)))
        .unwrap();
}

#[test]
fn concurrent_exact_user_retries_publish_one_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let (_second_manager, _second_scope, second) = open_store(temp.path(), "workspace-a");
    let barrier = Barrier::new(2);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| {
            barrier.wait();
            store.chat_append_user_message("alice", &c.id, &user("same", "hello"))
        });
        let b = s.spawn(|| {
            barrier.wait();
            second.chat_append_user_message("alice", &c.id, &user("same", "hello"))
        });
        (a.join().unwrap().unwrap(), b.join().unwrap().unwrap())
    });
    assert_eq!(a, b);
    assert_eq!(page(&store, &c).messages.len(), 1);
    assert_eq!(
        page(&store, &c)
            .conversation
            .last_user_message_id
            .as_deref(),
        Some(a.id.as_str())
    );
}

#[test]
fn insert_and_commit_failures_rollback_message_baseline_and_operation_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    sql(
        &store,
        "CREATE TRIGGER fail_chat_save BEFORE INSERT ON chat_messages BEGIN SELECT RAISE(ABORT,'injected save failure'); END;",
    );
    assert!(matches!(
        store.chat_append_user_message("alice", &c.id, &user("retry", "hello")),
        Err(ChatError::Storage(_))
    ));
    assert_eq!(page(&store, &c).conversation.last_user_message_id, None);
    assert!(page(&store, &c).messages.is_empty());
    sql(&store,"DROP TRIGGER fail_chat_save;
        CREATE TABLE chat_commit_failure (subject_id TEXT REFERENCES subjects(subject_id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER fail_chat_commit AFTER INSERT ON chat_messages BEGIN INSERT INTO chat_commit_failure VALUES ('missing'); END;");
    assert!(matches!(
        store.chat_append_user_message("alice", &c.id, &user("retry", "hello")),
        Err(ChatError::Storage(_))
    ));
    assert_eq!(page(&store, &c).conversation.last_user_message_id, None);
    assert!(page(&store, &c).messages.is_empty());
    assert!(matches!(
        store.chat_send_message(&subject, &send(&c, "retry", None)),
        Err(ChatError::Storage(_))
    ));
    sql(
        &store,
        "DROP TRIGGER fail_chat_commit; DROP TABLE chat_commit_failure;",
    );
    let first = store
        .chat_append_user_message("alice", &c.id, &user("retry", "hello"))
        .unwrap();
    assert_eq!(first.sequence, 1);
    assert_eq!(
        first.last_user_message_id.as_deref(),
        Some(first.id.as_str())
    );
    let answer = store
        .chat_send_message(&subject, &send(&c, "retry", Some(&first.id)))
        .unwrap();
    assert_eq!(answer.sequence, 2);
    assert_eq!(
        answer.last_user_message_id.as_deref(),
        Some(first.id.as_str())
    );
}

#[test]
fn subject_erasure_fences_replay_then_purges_chat_without_touching_other_subject() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let other = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let d = create(&store, "alice", &other, "other");
    let first = store
        .chat_append_user_message("alice", &c.id, &user("input", "erase me"))
        .unwrap();
    let mut request = send(&c, "reply", Some(&first.id));
    request.reply_to = Some(first.id);
    store.chat_send_message(&subject, &request).unwrap();
    let retained = store
        .chat_append_user_message("alice", &d.id, &user("input", "keep me"))
        .unwrap();
    store.begin_subject_deletion(&subject, "erase").unwrap();
    assert!(store.chat_conversation_for_account("alice", &c.id).is_err());
    assert!(
        store
            .chat_append_user_message("alice", &c.id, &user("input", "erase me"))
            .is_err()
    );
    assert!(store.chat_send_message(&subject, &request).is_err());
    assert!(
        store
            .chat_create_conversation(
                "alice",
                &ChatCreateConversationRequest {
                    operation_id: "create".into(),
                    subject_id: subject.clone()
                }
            )
            .is_err()
    );
    store.finish_subject_deletion(&subject, "erase").unwrap();
    store
        .database
        .with_connection(|db| {
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM chat_conversations", [], |r| r
                    .get::<_, u64>(0))?,
                1
            );
            assert_eq!(
                db.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r
                    .get::<_, u64>(0))?,
                1
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(page(&store, &d).messages, vec![retained]);
}

fn schema(store: &SubjektivStore) -> Vec<(String, String, String)> {
    store.database.with_connection(|db| {
        Ok(db.prepare("SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type,name")?
            .query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }).unwrap()
}

#[test]
fn oldest_supported_store_upgrades_to_fresh_schema_preserving_subjects_and_constraints() {
    let temp = tempfile::tempdir().unwrap();
    let subject = {
        let manager = FeatureStorage::new(temp.path());
        let scope = manager.workspace("upgraded").unwrap();
        let registered = scope
            .register(FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                &crate::MIGRATIONS[..1],
            ))
            .unwrap();
        let old = SubjektivStore::open(&scope, &registered).unwrap();
        let subject = crate::SubjectRecord {
            schema_version: crate::SUBJEKTIV_SCHEMA_VERSION,
            id: "preserved-subject".into(),
            role: role(),
            behavior_md: String::new(),
            state: crate::SubjectState::Active,
            created_at: crate::now(),
            updated_at: crate::now(),
        };
        old.database
            .with_connection(|db| {
                db.execute(
                    "INSERT INTO subjects VALUES (?1,?2,'active',0,?3,?4,?5)",
                    params![
                        subject.id,
                        subject.role.as_str(),
                        serde_json::to_string(&subject).unwrap(),
                        subject.created_at,
                        subject.updated_at
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        subject
    };
    let (_manager, _scope, upgraded) = open_store(temp.path(), "upgraded");
    let (_fresh_manager, _fresh_scope, fresh) = open_store(temp.path(), "fresh");
    assert_eq!(
        upgraded.subject(&subject.id).unwrap(),
        Some(subject.clone())
    );
    assert_eq!(schema(&upgraded), schema(&fresh));
    let c = create(&upgraded, "alice", &subject.id, "create");
    upgraded
        .chat_append_user_message("alice", &c.id, &user("input", "preserved subject"))
        .unwrap();
    let write = upgraded.database.with_connection(|db| {
        db.execute("UPDATE chat_messages SET text='tampered'", [])?;
        Ok(())
    });
    assert!(write.is_err());
    let delete = upgraded.database.with_connection(|db| {
        db.execute("DELETE FROM chat_messages", [])?;
        Ok(())
    });
    assert!(delete.is_err());
    assert_eq!(page(&upgraded, &c).messages[0].text, "preserved subject");
}

#[test]
fn failed_chat_migration_rolls_back_all_ddl_and_can_be_retried() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE subjects(subject_id TEXT PRIMARY KEY); CREATE TABLE subject_deletions(subject_id TEXT PRIMARY KEY); CREATE TABLE chat_messages(blocker TEXT);").unwrap();
    {
        let tx = connection.unchecked_transaction().unwrap();
        assert!(migrate(&tx).is_err());
    }
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='chat_conversations')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!exists);
    connection
        .execute_batch("DROP TABLE chat_messages;")
        .unwrap();
    let tx = connection.unchecked_transaction().unwrap();
    migrate(&tx).unwrap();
    tx.commit().unwrap();
    assert!(
        connection
            .execute(
                "INSERT INTO chat_conversations VALUES ('c','missing','a',NULL,'op')",
                []
            )
            .is_err()
    );
}

fn policy(
    store: &SubjektivStore,
    subject: &str,
    operation: &str,
    expected: Option<&str>,
) -> String {
    store
        .set_subject_permissions(
            subject,
            &server_api::authorization::SubjectPermissionsUpdate {
                operation_id: operation.into(),
                expected_change_id: expected.map(str::to_owned),
                execution: Vec::new(),
                delegation: Vec::new(),
            },
        )
        .unwrap()
        .change_id
}

#[test]
fn missing_or_replaced_policy_refuses_read_append_and_saved_replay() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let request = send(&c, "answer", None);
    let query = ChatMessagePageQuery::default();
    assert!(matches!(
        store.chat_messages_for_subject_authorized(&subject, &c.id, &query, "missing"),
        Err(ChatError::AuthorizationChanged)
    ));
    assert!(matches!(
        store.chat_send_message_authorized(&subject, &request, "missing"),
        Err(ChatError::AuthorizationChanged)
    ));
    let observed = policy(&store, &subject, "initial", None);
    let answer = store
        .chat_send_message_authorized(&subject, &request, &observed)
        .unwrap();
    assert_eq!(
        store
            .chat_messages_for_subject_authorized(&subject, &c.id, &query, &observed)
            .unwrap()
            .messages,
        vec![answer.clone()]
    );
    let (_other_manager, _other_scope, other) = open_store(temp.path(), "workspace-a");
    // An identical replacement still invalidates the Host's earlier observation.
    let current = policy(&other, &subject, "replace", Some(&observed));
    assert_ne!(current, observed);
    assert!(matches!(
        store.chat_messages_for_subject_authorized(&subject, &c.id, &query, &observed),
        Err(ChatError::AuthorizationChanged)
    ));
    assert!(matches!(
        store.chat_send_message_authorized(&subject, &request, &observed),
        Err(ChatError::AuthorizationChanged)
    ));
    assert!(matches!(
        store.chat_send_message_authorized(&subject, &send(&c, "new", None), &observed),
        Err(ChatError::AuthorizationChanged)
    ));
    assert_eq!(page(&store, &c).messages, vec![answer.clone()]);
    assert_eq!(
        store
            .chat_send_message_authorized(&subject, &request, &current)
            .unwrap(),
        answer
    );
}

#[test]
fn policy_replacement_and_send_serialize_across_sqlite_connections() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let observed = policy(&store, &subject, "initial", None);
    let (_other_manager, _other_scope, other) = open_store(temp.path(), "workspace-a");
    let barrier = Barrier::new(2);
    let (result, current) = std::thread::scope(|s| {
        let send = s.spawn(|| {
            barrier.wait();
            store.chat_send_message_authorized(&subject, &send(&c, "answer", None), &observed)
        });
        let replace = s.spawn(|| {
            barrier.wait();
            policy(&other, &subject, "replace", Some(&observed))
        });
        (send.join().unwrap(), replace.join().unwrap())
    });
    let saved = page(&store, &c).messages;
    match result {
        Ok(answer) => assert_eq!(saved, vec![answer]),
        Err(ChatError::AuthorizationChanged) => assert!(saved.is_empty()),
        other => panic!("unexpected policy race outcome: {other:?}"),
    }
    assert!(matches!(
        store.chat_send_message_authorized(&subject, &send(&c, "answer", None), &observed),
        Err(ChatError::AuthorizationChanged)
    ));
    assert!(matches!(
        store.chat_messages_for_subject_authorized(
            &subject,
            &c.id,
            &ChatMessagePageQuery::default(),
            &observed
        ),
        Err(ChatError::AuthorizationChanged)
    ));
    assert_eq!(
        store
            .chat_messages_for_subject_authorized(
                &subject,
                &c.id,
                &ChatMessagePageQuery::default(),
                &current
            )
            .unwrap()
            .messages,
        saved
    );
}

#[test]
fn null_baseline_replay_survives_user_input_and_reply_target_is_independent() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let opening = send(&c, "opening", None);
    let published = store.chat_send_message(&subject, &opening).unwrap();
    let first = store
        .chat_append_user_message("alice", &c.id, &user("first", "first"))
        .unwrap();
    let latest = store
        .chat_append_user_message("alice", &c.id, &user("latest", "new input"))
        .unwrap();
    assert_eq!(
        store.chat_send_message(&subject, &opening).unwrap(),
        published
    );
    let mut changed = opening.clone();
    changed.expected_last_user_message_id = Some(latest.id.clone());
    assert!(matches!(
        store.chat_send_message(&subject, &changed),
        Err(ChatError::OperationConflict(_))
    ));
    let mut request = send(&c, "reply", Some(&latest.id));
    request.reply_to = Some(first.id.clone());
    let answer = store.chat_send_message(&subject, &request).unwrap();
    assert_eq!(answer.reply_to, Some(first.id));
    assert_eq!(answer.last_user_message_id, Some(latest.id));
    // A subject-authored ID is never a user-input baseline.
    let bad = send(&c, "wrong", Some(&answer.id));
    assert!(
        matches!(store.chat_send_message(&subject, &bad), Err(ChatError::UserInputChanged { expected, actual }) if expected == Some(answer.id.clone()) && actual == answer.last_user_message_id)
    );
    assert_eq!(page(&store, &c).messages.len(), 4);
}

#[test]
fn schema_rejects_dangling_cross_conversation_or_subject_authored_user_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let other = create(&store, "alice", &subject, "other");
    let input = store
        .chat_append_user_message("alice", &other.id, &user("input", "other conversation"))
        .unwrap();
    let answer = store
        .chat_send_message(&subject, &send(&c, "answer", None))
        .unwrap();
    for target in [&input.id, &answer.id, "missing"] {
        let result = store.database.transaction(|tx| {
            tx.execute(
                "UPDATE chat_conversations SET last_user_message_id=?1 WHERE id=?2",
                params![target, c.id],
            )?;
            Ok(())
        });
        assert!(result.is_err(), "must reject user identity {target}");
    }
    let result = store.database.transaction(|tx| {
        tx.execute("UPDATE chat_conversations SET last_user_message_id='fresh-subject-message' WHERE id=?1", [&c.id])?;
        tx.execute("INSERT INTO chat_messages (id,conversation_id,sequence,sender_kind,sender_id,text,operation_id,last_user_message_id) VALUES ('fresh-subject-message',?1,2,'subject',?2,'bad','direct','fresh-subject-message')", params![c.id, subject])?;
        Ok(())
    });
    assert!(result.is_err());
    let result = store.database.transaction(|tx| {
        tx.execute("INSERT INTO chat_messages (id,conversation_id,sequence,sender_kind,sender_id,text,operation_id,last_user_message_id) VALUES ('bad-user',?1,2,'user','alice','bad','direct',NULL)", [&c.id])?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(page(&store, &c).conversation.last_user_message_id, None);
    assert_eq!(page(&store, &c).messages, vec![answer]);
}

#[test]
fn previous_schema_upgrade_preserves_policy_and_matches_fresh_database() {
    let temp = tempfile::tempdir().unwrap();
    let (subject, observed) = {
        let manager = FeatureStorage::new(temp.path());
        let scope = manager.workspace("upgraded").unwrap();
        let registered = scope
            .register(FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                &crate::MIGRATIONS[..14],
            ))
            .unwrap();
        let old = SubjektivStore::open(&scope, &registered).unwrap();
        let subject = old
            .create_subject_with_behavior(role(), "preserved behavior".into())
            .unwrap();
        let observed = policy(&old, &subject.id, "initial", None);
        (subject, observed)
    };
    let (_manager, _scope, upgraded) = open_store(temp.path(), "upgraded");
    let (_fresh_manager, _fresh_scope, fresh) = open_store(temp.path(), "fresh");
    assert_eq!(schema(&upgraded), schema(&fresh));
    assert_eq!(
        upgraded.subject(&subject.id).unwrap(),
        Some(subject.clone())
    );
    assert_eq!(
        upgraded
            .subject_permissions(&subject.id)
            .unwrap()
            .unwrap()
            .change_id,
        observed
    );
    let c = create(&upgraded, "alice", &subject.id, "create");
    let input = upgraded
        .chat_append_user_message("alice", &c.id, &user("input", "preserved subject"))
        .unwrap();
    upgraded
        .chat_send_message_authorized(&subject.id, &send(&c, "answer", Some(&input.id)), &observed)
        .unwrap();
    assert_eq!(
        page(&upgraded, &c).conversation.last_user_message_id,
        Some(input.id)
    );
}

#[test]
fn policy_replacement_and_read_serialize_across_sqlite_connections() {
    let temp = tempfile::tempdir().unwrap();
    let (_manager, _scope, store) = open_store(temp.path(), "workspace-a");
    let subject = store.create_subject(role()).unwrap().id;
    let c = create(&store, "alice", &subject, "create");
    let input = store
        .chat_append_user_message("alice", &c.id, &user("input", "private input"))
        .unwrap();
    let observed = policy(&store, &subject, "initial", None);
    let (_other_manager, _other_scope, other) = open_store(temp.path(), "workspace-a");
    let barrier = Barrier::new(2);
    let (result, current) = std::thread::scope(|s| {
        let read = s.spawn(|| {
            barrier.wait();
            store.chat_messages_for_subject_authorized(
                &subject,
                &c.id,
                &ChatMessagePageQuery::default(),
                &observed,
            )
        });
        let replace = s.spawn(|| {
            barrier.wait();
            policy(&other, &subject, "replace", Some(&observed))
        });
        (read.join().unwrap(), replace.join().unwrap())
    });
    match result {
        Ok(page) => assert_eq!(page.messages, vec![input.clone()]),
        Err(ChatError::AuthorizationChanged) => {}
        other => panic!("unexpected policy race outcome: {other:?}"),
    }
    assert!(matches!(
        store.chat_messages_for_subject_authorized(
            &subject,
            &c.id,
            &ChatMessagePageQuery::default(),
            &observed
        ),
        Err(ChatError::AuthorizationChanged)
    ));
    assert_eq!(
        store
            .chat_messages_for_subject_authorized(
                &subject,
                &c.id,
                &ChatMessagePageQuery::default(),
                &current
            )
            .unwrap()
            .messages,
        vec![input]
    );
}
