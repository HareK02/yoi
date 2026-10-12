use super::*;
use tempfile::tempdir;

fn create_tag(backend: &SqliteTicketBackend, key: &str, name: &str) -> WorkspaceTag {
    backend
        .mutate_workspace_tag(WorkspaceTagMutation {
            operation_key: key.into(),
            action: WorkspaceTagAction::Create { name: name.into() },
        })
        .unwrap()
}
fn rename_tag(
    backend: &SqliteTicketBackend,
    key: &str,
    tag: &WorkspaceTag,
    name: &str,
) -> WorkspaceTag {
    backend
        .mutate_workspace_tag(WorkspaceTagMutation {
            operation_key: key.into(),
            action: WorkspaceTagAction::Rename {
                tag_id: tag.id.clone(),
                expected_last_operation_key: tag.last_operation_key.clone(),
                name: name.into(),
            },
        })
        .unwrap()
}
fn delete_request(key: &str, tag: &WorkspaceTag) -> WorkspaceTagMutation {
    WorkspaceTagMutation {
        operation_key: key.into(),
        action: WorkspaceTagAction::Delete {
            tag_id: tag.id.clone(),
            expected_last_operation_key: tag.last_operation_key.clone(),
        },
    }
}
fn set_request(
    key: &str,
    expected: &[WorkspaceTag],
    desired: &[WorkspaceTag],
) -> TicketTagSetRequest {
    TicketTagSetRequest {
        expected_tag_ids: expected.iter().map(|t| t.id.clone()).collect(),
        tag_ids: desired.iter().map(|t| t.id.clone()).collect(),
        operation_key: key.into(),
    }
}
fn audit_count(backend: &SqliteTicketBackend) -> i64 {
    backend
        .open_connection()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM workspace_tag_operations WHERE workspace_id=?1",
            params![backend.workspace_id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn tag_names_have_documented_unicode_lowercase_not_casefold_policy() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let tag = create_tag(&backend, "a", "\u{2003}Ä BC\u{2003}");
    assert_eq!(tag.name, "Ä BC");
    for name in ["ä bc", " Ä BC "] {
        assert!(
            backend
                .mutate_workspace_tag(WorkspaceTagMutation {
                    operation_key: "duplicate".into(),
                    action: WorkspaceTagAction::Create { name: name.into() }
                })
                .is_err()
        );
    }
    for name in [
        "",
        " \u{2003}",
        "bad\nname",
        "\tname",
        "a\0",
        &"é".repeat(65),
    ] {
        assert!(
            backend
                .mutate_workspace_tag(WorkspaceTagMutation {
                    operation_key: "invalid".into(),
                    action: WorkspaceTagAction::Create { name: name.into() }
                })
                .is_err()
        );
    }
    create_tag(&backend, "boundary", &"é".repeat(64));
    create_tag(&backend, "sharp", "ß");
    create_tag(&backend, "ss", "SS"); // Lowercase, not full case folding.
    create_tag(&backend, "composed", "é");
    create_tag(&backend, "decomposed", "e\u{301}"); // No Unicode normalization.
    assert_eq!(audit_count(&backend), 6);
}

#[test]
fn tag_rename_delete_cas_and_name_conflicts_leave_no_audit_or_changes() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let a = create_tag(&backend, "a", "A");
    let b = create_tag(&backend, "b", "B");
    let renamed = rename_tag(&backend, "rename", &a, "New");
    assert_eq!(a.id, renamed.id);
    for action in [
        WorkspaceTagAction::Rename {
            tag_id: a.id.clone(),
            expected_last_operation_key: "a".into(),
            name: "Stale".into(),
        },
        WorkspaceTagAction::Delete {
            tag_id: a.id.clone(),
            expected_last_operation_key: "a".into(),
        },
        WorkspaceTagAction::Rename {
            tag_id: renamed.id.clone(),
            expected_last_operation_key: "rename".into(),
            name: " b ".into(),
        },
        WorkspaceTagAction::Rename {
            tag_id: renamed.id.clone(),
            expected_last_operation_key: "rename".into(),
            name: "\n".into(),
        },
    ] {
        assert!(
            backend
                .mutate_workspace_tag(WorkspaceTagMutation {
                    operation_key: "rejected".into(),
                    action
                })
                .is_err()
        );
    }
    assert_eq!(audit_count(&backend), 3);
    let active = backend
        .list_workspace_tags(TagListRequest::default())
        .unwrap();
    assert!(active.contains(&renamed) && active.contains(&b));
}

#[test]
fn workspace_mutation_exact_replay_returns_stored_snapshot_before_cas() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("tags.db");
    let backend = SqliteTicketBackend::open(&db, "ws")
        .unwrap()
        .with_event_attributes(BTreeMap::from([(
            "source_worker_id".into(),
            "actor-a".into(),
        )]));
    let request = WorkspaceTagMutation {
        operation_key: "create".into(),
        action: WorkspaceTagAction::Create { name: " A ".into() },
    };
    let original = backend.mutate_workspace_tag(request.clone()).unwrap();
    let renamed = rename_tag(&backend, "rename", &original, "B");
    let deletion = delete_request("delete", &renamed);
    let tombstone = backend.mutate_workspace_tag(deletion.clone()).unwrap();
    assert!(tombstone.deleted);
    assert_eq!(
        backend.mutate_workspace_tag(request.clone()).unwrap(),
        original
    );
    assert_eq!(
        backend.mutate_workspace_tag(deletion.clone()).unwrap(),
        tombstone
    );
    let restarted = SqliteTicketBackend::open_verified(&db, "ws")
        .unwrap()
        .with_event_attributes(backend.event_attributes.clone());
    assert_eq!(
        restarted.mutate_workspace_tag(request.clone()).unwrap(),
        original
    );
    let other_actor = restarted.with_event_attributes(BTreeMap::from([(
        "source_worker_id".into(),
        "actor-b".into(),
    )]));
    assert!(matches!(
        other_actor.mutate_workspace_tag(request),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert!(matches!(
        backend.mutate_workspace_tag(WorkspaceTagMutation {
            operation_key: "create".into(),
            action: WorkspaceTagAction::Create { name: "A".into() }
        }),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert_eq!(audit_count(&backend), 3);
    assert!(
        backend
            .list_workspace_tags(TagListRequest::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tag_deletion_detaches_every_ticket_keeps_audit_snapshots_and_recreate_new_id() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let tag = create_tag(&backend, "create", "Old name");
    let mut ids = Vec::new();
    for i in 0..2 {
        let id = backend
            .create(NewTicket::new(format!("ticket {i}")))
            .unwrap()
            .id;
        backend
            .set_ticket_tags(
                id.clone().into(),
                set_request(&format!("attach-{i}"), &[], std::slice::from_ref(&tag)),
            )
            .unwrap();
        ids.push(id);
    }
    let renamed = rename_tag(&backend, "rename", &tag, "New name");
    backend
        .mutate_workspace_tag(delete_request("delete", &renamed))
        .unwrap();
    let replacement = create_tag(&backend, "recreate", "new NAME");
    assert_ne!(replacement.id, tag.id);
    for id in ids {
        let ticket = backend.show(id.into()).unwrap();
        assert!(ticket.tags.is_empty());
        let events: Vec<_> = ticket
            .events
            .iter()
            .filter(|e| e.kind == TicketEventKind::TagsChanged)
            .collect();
        assert_eq!(events.len(), 2);
        let first: Vec<WorkspaceTag> =
            serde_json::from_str(&events[0].attributes["tags_after"]).unwrap();
        assert_eq!(first, vec![tag.clone()]);
        let before_delete: Vec<WorkspaceTag> =
            serde_json::from_str(&events[1].attributes["tags_before"]).unwrap();
        assert_eq!(before_delete, vec![renamed.clone()]);
        assert_eq!(events[1].attributes["tags_after"], "[]");
    }
    let conn = backend.open_connection().unwrap();
    let deleted: i64 = conn
        .query_row(
            "SELECT deleted FROM workspace_tags WHERE workspace_id='ws' AND tag_id=?1",
            params![tag.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(audit_count(&backend), 6);
}

#[test]
fn ticket_tag_set_compares_whole_set_validates_ids_and_replays_historical_result() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let a = create_tag(&backend, "a", "A");
    let b = create_tag(&backend, "b", "B");
    let id = backend.create(NewTicket::new("ticket")).unwrap().id;
    let request = set_request("set", &[], &[b.clone(), a.clone()]);
    let stored = backend
        .set_ticket_tags(id.clone().into(), request.clone())
        .unwrap();
    assert!(stored.windows(2).all(|pair| pair[0].id < pair[1].id));
    for bad in [
        set_request("bad", std::slice::from_ref(&a), &[]),
        TicketTagSetRequest {
            expected_tag_ids: stored.iter().map(|t| t.id.clone()).collect(),
            tag_ids: vec![a.id.clone(), a.id.clone()],
            operation_key: "bad".into(),
        },
        TicketTagSetRequest {
            expected_tag_ids: stored.iter().map(|t| t.id.clone()).collect(),
            tag_ids: vec!["missing".into()],
            operation_key: "bad".into(),
        },
    ] {
        assert!(backend.set_ticket_tags(id.clone().into(), bad).is_err());
    }
    assert_eq!(backend.show(id.clone().into()).unwrap().tags, stored);
    let renamed = rename_tag(&backend, "rename", &a, "Renamed");
    backend
        .mutate_workspace_tag(delete_request("delete", &renamed))
        .unwrap();
    assert_eq!(
        backend
            .set_ticket_tags(id.clone().into(), request.clone())
            .unwrap(),
        stored
    );
    let mut changed = request.clone();
    changed.tag_ids.reverse();
    assert!(matches!(
        backend.set_ticket_tags(id.clone().into(), changed),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    let current = backend.show(id.clone().into()).unwrap();
    assert_eq!(current.tags, vec![b]);
    assert_eq!(
        current
            .events
            .iter()
            .filter(|e| e.kind == TicketEventKind::TagsChanged)
            .count(),
        2
    );
    let other_actor = backend.clone().with_event_attributes(BTreeMap::from([(
        "source_worker_id".into(),
        "actor-b".into(),
    )]));
    assert!(matches!(
        other_actor.set_ticket_tags(id.into(), request),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert_eq!(audit_count(&backend), 5);
}

#[test]
fn tags_and_operation_keys_are_workspace_scoped() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("tags.db");
    let one = SqliteTicketBackend::open(&db, "one").unwrap();
    let two = SqliteTicketBackend::open(&db, "two").unwrap();
    let a = create_tag(&one, "same-key", "Same name");
    let b = create_tag(&two, "same-key", "Same name");
    let ticket = two.create(NewTicket::new("ticket")).unwrap().id;
    assert!(
        two.mutate_workspace_tag(delete_request("delete", &a))
            .is_err()
    );
    assert!(
        two.set_ticket_tags(ticket.clone().into(), set_request("attach", &[], &[a]))
            .is_err()
    );
    two.set_ticket_tags(
        ticket.into(),
        set_request("attach", &[], std::slice::from_ref(&b)),
    )
    .unwrap();
    assert_eq!(
        one.list_workspace_tags(TagListRequest::default())
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        two.list_workspace_tags(TagListRequest::default()).unwrap(),
        vec![b]
    );
    assert_eq!(audit_count(&one), 1);
}

#[test]
fn tags_do_not_change_content_workflow_queue_targets_or_legacy_labels_in_any_state() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let tag = create_tag(&backend, "tag", "open");
    let closed_tag = create_tag(&backend, "closed-tag", "closed");
    for (i, state) in [TicketState::Open, TicketState::Closed]
        .into_iter()
        .enumerate()
    {
        let mut new = NewTicket::new(format!("ticket {i}"));
        new.workflow_state = Some(state);
        new.labels = vec!["legacy label".into()];
        new.queued_by = Some("user".into());
        new.queued_at = Some("yesterday".into());
        let id = backend.create(new).unwrap().id;
        let before = backend.show(id.clone().into()).unwrap();
        backend
            .set_ticket_tags(
                id.clone().into(),
                set_request(&format!("set-{i}"), &[], &[tag.clone(), closed_tag.clone()]),
            )
            .unwrap();
        let after = backend.show(id.clone().into()).unwrap();
        assert_eq!(before.meta, after.meta);
        assert_eq!(before.document, after.document);
        assert_eq!(before.resolution, after.resolution);
        assert_eq!(
            ticket_content_digest(&before),
            ticket_content_digest(&after)
        );
        assert_eq!(before.closure_operation_key, after.closure_operation_key);
        assert_eq!(before.current_closure, after.current_closure);
        assert_eq!(before.case_history, after.case_history);
        assert_eq!(
            before.legacy_workflow_snapshot,
            after.legacy_workflow_snapshot
        );
        let mut expected_tags = vec![tag.clone(), closed_tag.clone()];
        expected_tags.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(after.tags, expected_tags);
        let summary = backend
            .list(TicketListQuery::all())
            .unwrap()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap();
        assert_eq!(summary.tags, after.tags);
        let projection = backend.list_workspace_projection(200).unwrap();
        assert_eq!(
            projection
                .items
                .iter()
                .find(|s| s.summary.id == id)
                .unwrap()
                .summary
                .tags,
            after.tags
        );
        let page = backend
            .list_workspace_projection_page(SqliteTicketListPageQuery {
                limit: 200,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .find(|s| s.summary.id == id)
                .unwrap()
                .summary
                .tags,
            after.tags
        );
    }
}

#[test]
fn tag_list_has_bounded_id_keyset_order_and_excludes_deleted() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    // Fixture insertion avoids spending this query test on 205 mutation transactions.
    backend
        .with_write(|conn| {
            for i in 0..205 {
                conn.execute(
                    "INSERT INTO workspace_tags VALUES ('ws',?1,?1,?1,'fixture',0)",
                    params![format!("tag-{i:03}")],
                )
                .map_err(sqlite_err)?;
            }
            Ok(())
        })
        .unwrap();
    let first = backend
        .list_workspace_tags(TagListRequest {
            after: None,
            limit: Some(usize::MAX),
        })
        .unwrap();
    assert_eq!(first.len(), 200);
    assert_eq!(first[0].id, "tag-000");
    assert_eq!(first[199].id, "tag-199");
    assert_eq!(
        backend
            .list_workspace_tags(TagListRequest::default())
            .unwrap(),
        first
    );
    backend
        .mutate_workspace_tag(delete_request("delete", &first[0]))
        .unwrap();
    let tail = backend
        .list_workspace_tags(TagListRequest {
            after: Some("tag-199".into()),
            limit: Some(200),
        })
        .unwrap();
    assert_eq!(tail.len(), 5);
    assert!(
        backend
            .list_workspace_tags(TagListRequest {
                after: None,
                limit: Some(0)
            })
            .unwrap()
            .is_empty()
    );
    assert!(
        backend
            .list_workspace_tags(TagListRequest {
                after: None,
                limit: Some(1)
            })
            .unwrap()
            .iter()
            .all(|t| t.id == "tag-001")
    );
}

#[test]
fn failed_tag_mutation_hook_rolls_back_links_tombstone_audit_and_receipt() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let tag = create_tag(&backend, "create", "Tag");
    let id = backend.create(NewTicket::new("ticket")).unwrap().id;
    let failing = backend.clone().with_mutation_hook(Arc::new(|_, _| {
        Err(TicketError::Conflict("test hook".into()))
    }));
    let attach = set_request("attach", &[], std::slice::from_ref(&tag));
    assert!(
        failing
            .set_ticket_tags(id.clone().into(), attach.clone())
            .is_err()
    );
    assert!(backend.show(id.clone().into()).unwrap().tags.is_empty());
    assert_eq!(audit_count(&backend), 1);
    backend.set_ticket_tags(id.clone().into(), attach).unwrap();
    let before = backend.show(id.clone().into()).unwrap();
    let deletion = delete_request("delete", &tag);
    assert!(failing.mutate_workspace_tag(deletion.clone()).is_err());
    assert_eq!(backend.show(id.into()).unwrap(), before);
    assert_eq!(
        backend
            .list_workspace_tags(TagListRequest::default())
            .unwrap(),
        vec![tag]
    );
    assert_eq!(audit_count(&backend), 2);
    backend.mutate_workspace_tag(deletion).unwrap();
    assert_eq!(audit_count(&backend), 3);
}

#[test]
fn concurrent_tag_replay_and_delete_attach_are_serialized() {
    use std::sync::Barrier;
    use std::thread;
    let dir = tempdir().unwrap();
    let db = dir.path().join("tags.db");
    let backend = SqliteTicketBackend::open(&db, "ws").unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut joins = Vec::new();
    for _ in 0..2 {
        let backend = backend.clone();
        let barrier = barrier.clone();
        joins.push(thread::spawn(move || {
            barrier.wait();
            create_tag(&backend, "create", "Tag")
        }));
    }
    barrier.wait();
    let tag = joins.remove(0).join().unwrap();
    assert_eq!(joins.remove(0).join().unwrap(), tag);
    assert_eq!(audit_count(&backend), 1);
    let id = backend.create(NewTicket::new("ticket")).unwrap().id;
    let barrier = Arc::new(Barrier::new(3));
    let attach = {
        let backend = backend.clone();
        let barrier = barrier.clone();
        let tag = tag.clone();
        let id = id.clone();
        thread::spawn(move || {
            barrier.wait();
            backend.set_ticket_tags(id.into(), set_request("attach", &[], &[tag]))
        })
    };
    let delete = {
        let backend = backend.clone();
        let barrier = barrier.clone();
        let tag = tag.clone();
        thread::spawn(move || {
            barrier.wait();
            backend.mutate_workspace_tag(delete_request("delete", &tag))
        })
    };
    barrier.wait();
    let attached = attach.join().unwrap();
    assert!(delete.join().unwrap().unwrap().deleted);
    assert!(backend.show(id.into()).unwrap().tags.is_empty());
    assert!(
        backend
            .list_workspace_tags(TagListRequest::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(audit_count(&backend), if attached.is_ok() { 3 } else { 2 });
    let links: i64 = backend
        .open_connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM typed_ticket_tags", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(links, 0);
}

#[test]
fn ticket_tag_receipts_bind_selector_and_operation_kind_and_replay_before_assignment_fence() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let tag = create_tag(&backend, "create", "Tag");
    let id = backend.create(NewTicket::new("ticket")).unwrap().id;
    let request = set_request("attach", &[], std::slice::from_ref(&tag));
    let mut reused = request.clone();
    reused.operation_key = "create".into();
    assert!(matches!(
        backend.set_ticket_tags(id.clone().into(), reused),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    let conn = backend.open_connection().unwrap();
    conn.execute_batch("CREATE TABLE ticket_active_worker_assignments (workspace_id TEXT,ticket_id TEXT,runtime_id TEXT,worker_id TEXT,assignment_id TEXT);").unwrap();
    conn.execute("INSERT INTO ticket_active_worker_assignments VALUES ('ws',?1,'runtime','worker','assignment')",params![id]).unwrap();
    let worker = backend.clone().with_event_attributes(BTreeMap::from([
        ("source_actor_role".into(), "ticket_worker".into()),
        ("source_runtime_id".into(), "runtime".into()),
        ("source_worker_id".into(), "worker".into()),
        ("source_assignment_id".into(), "assignment".into()),
    ]));
    let stored = worker
        .set_ticket_tags(TicketIdOrSlug::Id(id.clone()), request.clone())
        .unwrap();
    conn.execute("DELETE FROM ticket_active_worker_assignments", [])
        .unwrap();
    assert_eq!(
        worker
            .set_ticket_tags(TicketIdOrSlug::Id(id.clone()), request.clone())
            .unwrap(),
        stored
    );
    assert!(matches!(
        worker.set_ticket_tags(TicketIdOrSlug::Query(id.clone()), request),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert!(
        worker
            .set_ticket_tags(
                id.clone().into(),
                set_request("new-operation", &stored, &[])
            )
            .is_err()
    );
    assert_eq!(backend.show(id.clone().into()).unwrap().tags, stored);
    backend
        .mutate_workspace_tag(delete_request("delete", &tag))
        .unwrap();
    assert!(
        backend
            .set_ticket_tags(
                id.into(),
                set_request("deleted-attach", &[], std::slice::from_ref(&tag))
            )
            .is_err()
    );
    assert_eq!(audit_count(&backend), 3);
}

#[test]
fn tag_domain_dispatch_and_wire_contracts_are_exact() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let request = WorkspaceTagMutation {
        operation_key: "create".into(),
        action: WorkspaceTagAction::Create {
            name: "Name".into(),
        },
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap(),
        serde_json::json!({"operation_key":"create","action":{"kind":"create","name":"Name"}})
    );
    let operation = TicketBackendOperation::MutateWorkspaceTag { request };
    let operation = serde_json::from_value(serde_json::to_value(operation).unwrap()).unwrap();
    let TicketBackendOperationResult::WorkspaceTag(tag) =
        execute_ticket_backend_operation(&backend, operation).unwrap()
    else {
        panic!("tag result")
    };
    let id = backend.create(NewTicket::new("ticket")).unwrap().id;
    let result = execute_ticket_backend_operation(
        &backend,
        TicketBackendOperation::SetTicketTags {
            id: id.clone().into(),
            request: set_request("set", &[], std::slice::from_ref(&tag)),
        },
    )
    .unwrap();
    assert!(
        matches!(result,TicketBackendOperationResult::WorkspaceTags(tags) if tags==vec![tag.clone()])
    );
    let result = execute_ticket_backend_operation(
        &backend,
        TicketBackendOperation::ListWorkspaceTags {
            request: TagListRequest::default(),
        },
    )
    .unwrap();
    assert!(matches!(result,TicketBackendOperationResult::WorkspaceTags(tags) if tags==vec![tag]));
    let ticket = backend.show(id.into()).unwrap();
    let mut wire = serde_json::to_value(&ticket).unwrap();
    wire.as_object_mut().unwrap().remove("tags");
    assert!(
        serde_json::from_value::<Ticket>(wire)
            .unwrap()
            .tags
            .is_empty()
    );
    let summary = backend.list(TicketListQuery::all()).unwrap().remove(0);
    let mut wire = serde_json::to_value(summary).unwrap();
    wire.as_object_mut().unwrap().remove("tags");
    assert!(
        serde_json::from_value::<TicketSummary>(wire)
            .unwrap()
            .tags
            .is_empty()
    );
    let schema = schemars::schema_for!(WorkspaceTagAction);
    assert!(
        serde_json::to_string(&schema)
            .unwrap()
            .contains("expected_last_operation_key")
    );
}

#[test]
fn unchanged_tag_set_records_receipt_without_claiming_a_membership_change() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let id = backend.create(NewTicket::new("ticket")).unwrap().id;
    let before = backend.show(id.clone().into()).unwrap();
    let request = set_request("noop", &[], &[]);
    assert!(
        backend
            .set_ticket_tags(id.clone().into(), request.clone())
            .unwrap()
            .is_empty()
    );
    assert!(
        backend
            .set_ticket_tags(id.clone().into(), request)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        backend.show(id.clone().into()).unwrap().events,
        before.events
    );
    assert_eq!(audit_count(&backend), 1);
    for request in [
        TicketTagSetRequest {
            expected_tag_ids: vec!["x".into(); 101],
            tag_ids: vec![],
            operation_key: "large-observation".into(),
        },
        TicketTagSetRequest {
            expected_tag_ids: vec![],
            tag_ids: (0..101).map(|i| format!("tag-{i}")).collect(),
            operation_key: "large-desired".into(),
        },
        TicketTagSetRequest {
            expected_tag_ids: vec![],
            tag_ids: vec!["x".repeat(129)],
            operation_key: "large-id".into(),
        },
    ] {
        assert!(backend.set_ticket_tags(id.clone().into(), request).is_err());
        assert_eq!(
            backend.show(id.clone().into()).unwrap().events,
            before.events
        );
        assert_eq!(audit_count(&backend), 1);
    }
}

#[test]
fn tag_receipt_preserves_original_context_but_reassignment_does_not_change_request_identity() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("tags.db"), "ws").unwrap();
    let original_context = BTreeMap::from([
        ("source_runtime_id".into(), "runtime".into()),
        ("source_worker_id".into(), "worker".into()),
        ("source_actor_role".into(), "ticket_worker".into()),
        ("source_assignment_id".into(), "assignment-a".into()),
    ]);
    let original = backend
        .clone()
        .with_event_attributes(original_context.clone());
    let request = WorkspaceTagMutation {
        operation_key: "create".into(),
        action: WorkspaceTagAction::Create {
            name: "ready".into(),
        },
    };
    let tag = original.mutate_workspace_tag(request.clone()).unwrap();
    let reassigned = backend.clone().with_event_attributes(BTreeMap::from([
        ("source_runtime_id".into(), "runtime".into()),
        ("source_worker_id".into(), "worker".into()),
        ("source_actor_role".into(), "contributor".into()),
    ]));
    assert_eq!(reassigned.mutate_workspace_tag(request).unwrap(), tag);
    let stored: String = backend.with_read(|conn| conn.query_row("SELECT source_context_json FROM workspace_tag_operations WHERE workspace_id='ws' AND operation_key='create'", [], |row| row.get(0)).map_err(sqlite_err)).unwrap();
    assert_eq!(
        serde_json::from_str::<BTreeMap<String, String>>(&stored).unwrap(),
        original_context
    );
    assert_eq!(audit_count(&backend), 1);
}
