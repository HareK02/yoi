use super::*;
use crate::authority::TicketAuthority;
use crate::store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};
use rusqlite::params;
use ticket::{SqliteTicketBackend, TicketBackend, TicketClosureReason, TicketState};

struct Fixture {
    _dir: tempfile::TempDir,
    authority: SqliteWorkspaceAuthority,
    backend: SqliteTicketBackend,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browse.db");
        let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
        for workspace in ["w", "foreign"] {
            store
                .upsert_workspace(&WorkspaceRecord {
                    workspace_id: workspace.into(),
                    owner_account_id: "owner-account".into(),
                    display_name: workspace.into(),
                    state: "active".into(),
                    created_at: "1".into(),
                    updated_at: "1".into(),
                })
                .await
                .unwrap();
        }
        Self {
            authority: SqliteWorkspaceAuthority::new(&path, "w").unwrap(),
            backend: SqliteTicketBackend::open_verified(&path, "w").unwrap(),
            _dir: dir,
        }
    }
    fn ticket(&self, title: &str, time: Option<&str>) -> String {
        let id = self
            .backend
            .create(ticket::NewTicket::new(title))
            .unwrap()
            .id;
        self.authority.store.with_conn(|conn| {
            conn.execute("UPDATE typed_tickets SET updated_at=?1 WHERE workspace_id='w' AND ticket_id=?2", params![time,id])?;
            Ok(())
        }).unwrap();
        id
    }
    fn run(&self, query: TicketBrowseQueryRequest) -> TicketBrowseQueryResponse {
        self.authority.query_ticket_browse(query).unwrap()
    }
    fn tag(&self, ticket: &str, tag: &str) {
        self.authority.store.with_conn(|conn| {
            conn.execute("INSERT OR IGNORE INTO workspace_tags(workspace_id,tag_id,name,normalized_name,last_operation_key,deleted) VALUES('w',?1,?1,?1,?1,0)",[tag])?;
            conn.execute("INSERT INTO typed_ticket_tags(workspace_id,ticket_id,tag_id) VALUES('w',?1,?2)",params![ticket,tag])?;
            Ok(())
        }).unwrap();
    }
    fn closure(&self, id: &str, operation: &str, reason: TicketClosureReason, closed: bool) {
        let entry = ticket::TicketCaseHistoryEntry {
            operation_key: operation.into(),
            previous_closure_operation_key: None,
            action: ticket::TicketCaseAction::Close { reason },
            explanation: "test judgment".into(),
            references: vec![],
            author: "owner-account".into(),
            at: "1".into(),
        };
        self.authority.store.with_conn(|conn| {
            conn.execute("INSERT INTO typed_ticket_case_history(workspace_id,ticket_id,ordinal,operation_key,request_fingerprint,judgment_json)
                SELECT 'w',?1,coalesce(max(ordinal),0)+1,?2,'fixture',?3 FROM typed_ticket_case_history WHERE workspace_id='w' AND ticket_id=?1",
                params![id,operation,serde_json::to_string(&entry).unwrap()])?;
            conn.execute("INSERT INTO typed_ticket_case_metadata(workspace_id,ticket_id,closure_operation_key) VALUES('w',?1,?2)
                ON CONFLICT(workspace_id,ticket_id) DO UPDATE SET closure_operation_key=excluded.closure_operation_key",params![id,operation])?;
            conn.execute("UPDATE typed_tickets SET workflow_state=?1 WHERE workspace_id='w' AND ticket_id=?2",params![if closed {"closed"} else {"open"},id])?;
            Ok(())
        }).unwrap();
    }
    fn assign(&self, id: &str, assignment: &str, status: Option<&str>, current: bool) {
        self.authority.store.with_conn(|conn| {
            conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at)
                VALUES('w',?1,?2,'orchestrator','subject','subject-a','owner-account','1')",params![id,assignment])?;
            conn.execute("UPDATE ticket_worker_assignments SET tracking_json=?1,tracking_updated_at=?2 WHERE workspace_id='w' AND assignment_id=?3",
                params![status.map(|s| serde_json::json!({"status":s,"waiting_reason":null,"progress":null,"next_action":null,"result":null}).to_string()),status.map(|_|"1"),assignment])?;
            if current {
                conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,role,assignment_id,principal_kind,principal_id,updated_at)
                    VALUES('w',?1,'orchestrator',?2,'subject','subject-a','1')",params![id,assignment])?;
            }
            Ok(())
        }).unwrap();
    }
    fn event(&self, id: &str, index: i64, body: &str, archived: bool) {
        self.authority.store.with_conn(|conn| {
            if index == -1 {
                conn.execute("UPDATE typed_ticket_events SET body=?1 WHERE workspace_id='w' AND ticket_id=?2 AND event_index=-1",params![body,id])?;
            } else {
                conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body,event_ref,archived)
                    VALUES('w',?1,?2,'comment',?3,?4,?5)",params![id,index,body,format!("{id}-event-{index}"),archived])?;
            }
            Ok(())
        }).unwrap();
    }
}
fn assigned_subject() -> ticket::TicketAssignmentPrincipalSnapshot {
    ticket::TicketAssignmentPrincipalSnapshot::Subject {
        subject_id: "subject-a".into(),
    }
}
fn ids(response: &TicketBrowseQueryResponse) -> Vec<String> {
    response.items.iter().map(|item| item.id.clone()).collect()
}

#[tokio::test]
async fn ticket_browse_unicode_search_uses_same_case_mapping_before_pagination() {
    let f = Fixture::new().await;
    f.ticket("newer nonmatch", Some("9"));
    let title = f.ticket("ÉCOLE", Some("4"));
    let body = f.ticket("body", Some("3"));
    f.event(&body, -1, "ÜBER", false);
    let thread = f.ticket("thread", Some("2"));
    f.event(&thread, 0, "ΜΉΝΥΜΑ", false);
    let closure = f.ticket("closure", Some("1"));
    f.closure(
        &closure,
        "unicode-close",
        TicketClosureReason::Other("ÖFFEN".into()),
        true,
    );
    let special = f.ticket("special", Some("0"));
    f.event(&special, 0, "ascii\0ÜNIT Kelvin", false);
    for (needle, expected) in [
        ("ünit", &special),
        ("kelvin", &special),
        ("ÉCOLE", &title),
        ("école", &title),
        ("ÜBER", &body),
        ("über", &body),
        ("ΜΉΝΥΜΑ", &thread),
        ("μήνυμα", &thread),
        ("ÖFFEN", &closure),
        ("öffen", &closure),
    ] {
        let response = f.run(TicketBrowseQueryRequest {
            query: Some(needle.into()),
            limit: Some(1),
            ..Default::default()
        });
        assert_eq!(ids(&response), vec![expected.clone()], "{needle}");
        assert!(!response.page.has_more);
    }
}

#[tokio::test]
async fn ticket_browse_keyset_ties_null_dates_and_scoped_canonical_cursors() {
    let f = Fixture::new().await;
    let mut tied = vec![
        f.ticket("tie 1", Some("2")),
        f.ticket("tie 2", Some("2")),
        f.ticket("tie 3", Some("2")),
    ];
    tied.sort();
    let newer = f.ticket("new", Some("3"));
    let older = f.ticket("old", Some("1"));
    let empty = f.ticket("empty", Some(""));
    let mut nulls = vec![f.ticket("null 1", None), f.ticket("null 2", None)];
    nulls.sort();
    let mut expected = vec![newer];
    expected.extend(tied);
    expected.extend([older, empty]);
    expected.extend(nulls);
    let mut query = TicketBrowseQueryRequest {
        states: vec![TicketState::Closed, TicketState::Open, TicketState::Open],
        limit: Some(2),
        ..Default::default()
    };
    let first = f.run(query.clone());
    assert_eq!(first.page.returned, 2);
    assert!(first.page.has_more);
    assert!(!first.page.source_truncated);
    assert_eq!(first.page.source_limit, None);
    let cursor = first.page.next_cursor.clone();
    let mut seen = ids(&first);
    query.states = vec![TicketState::Open, TicketState::Closed];
    query.sort = Some("updated_desc".into());
    query.limit = Some(3);
    query.cursor = cursor.clone();
    loop {
        let page = f.run(query.clone());
        seen.extend(ids(&page));
        if !page.page.has_more {
            assert!(page.page.next_cursor.is_none());
            break;
        }
        assert_eq!(page.items.len(), 3);
        query.cursor = page.page.next_cursor;
    }
    assert_eq!(seen, expected);
    let changed = TicketBrowseQueryRequest {
        cursor: cursor.clone(),
        states: vec![TicketState::Closed],
        ..Default::default()
    };
    assert!(f.authority.query_ticket_browse(changed).is_err());
    let foreign =
        SqliteWorkspaceAuthority::new(f._dir.path().join("browse.db"), "foreign").unwrap();
    assert!(
        foreign
            .query_ticket_browse(TicketBrowseQueryRequest {
                cursor: cursor.clone(),
                states: vec![TicketState::Open, TicketState::Closed],
                ..Default::default()
            })
            .is_err()
    );
    assert!(
        f.authority
            .query_tickets(server_api::TicketQueryRequest {
                cursor,
                ..Default::default()
            })
            .is_err()
    );
    for sort in ["title", "relevance", "created_desc"] {
        assert!(
            f.authority
                .query_ticket_browse(TicketBrowseQueryRequest {
                    sort: Some(sort.into()),
                    ..Default::default()
                })
                .is_err()
        );
    }
    assert!(
        f.authority
            .query_ticket_browse(TicketBrowseQueryRequest {
                cursor: Some("v1:garbage".into()),
                ..Default::default()
            })
            .is_err()
    );
}

#[tokio::test]
async fn ticket_browse_all_filters_are_applied_before_limit_and_batch_fields_match() {
    let f = Fixture::new().await;
    for i in 0..8 {
        f.ticket(&format!("irrelevant {i}"), Some("9"));
    }
    let one = f.ticket("Needle one", Some("2"));
    let two = f.ticket("Needle two", Some("1"));
    let partial = f.ticket("Needle partial", Some("8"));
    for id in [&one, &two, &partial] {
        f.tag(id, "a");
        f.closure(
            id,
            &format!("close-{id}"),
            TicketClosureReason::Other("custom reason".into()),
            true,
        );
        f.assign(id, &format!("assignment-{id}"), Some("done"), true);
    }
    f.tag(&one, "b");
    f.tag(&two, "b");
    let query = TicketBrowseQueryRequest {
        query: Some("NEEDLE".into()),
        states: vec![TicketState::Closed],
        tag_ids: vec!["b".into(), "a".into(), "a".into()],
        assigned_principal: Some(assigned_subject()),
        handling_status: Some(ticket::TicketAssignmentTrackingStatus::Done),
        closure_reason: Some(TicketClosureReason::Other("custom reason".into())),
        limit: Some(1),
        ..Default::default()
    };
    let first = f.run(query.clone());
    assert_eq!(ids(&first), vec![one.clone()]);
    assert!(first.page.has_more);
    assert_eq!(
        first.items[0]
            .tags
            .iter()
            .map(|tag| tag.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    let detail = f.authority.ticket(&one).unwrap();
    assert_eq!(
        first.items[0].assignment_tracking,
        detail.assignment_tracking
    );
    assert_eq!(first.items[0].current_closure, detail.current_closure);
    let second = f.run(TicketBrowseQueryRequest {
        cursor: first.page.next_cursor,
        tag_ids: vec!["a".into(), "b".into()],
        query: Some("needle".into()),
        ..query.clone()
    });
    assert_eq!(ids(&second), vec![two]);
    assert!(!second.page.has_more);
    for missing in ["missing", "deleted"] {
        assert!(
            f.run(TicketBrowseQueryRequest {
                tag_ids: vec![missing.into()],
                ..query.clone()
            })
            .items
            .is_empty()
        );
    }
    f.authority
        .store
        .with_conn(|conn| {
            conn.execute(
                "UPDATE workspace_tags SET deleted=1 WHERE workspace_id='w' AND tag_id='b'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(f.run(query).items.is_empty());
}

#[tokio::test]
async fn ticket_browse_assignment_current_missing_unknown_and_cleared_semantics() {
    let f = Fixture::new().await;
    let unassigned = f.ticket("unassigned", Some("1"));
    let unknown = f.ticket("unknown", Some("1"));
    let cleared = f.ticket("cleared", Some("1"));
    let current = f.ticket("current", Some("1"));
    f.assign(&unknown, "unknown", None, true);
    f.authority.store.with_conn(|conn| {
        conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at) VALUES('w',?1,'orchestrator','legacy-assign','assigned','unknown','owner-account','1')",[&unknown])?;
        Ok(())
    }).unwrap();
    f.assign(&cleared, "cleared", Some("done"), true);
    f.authority.store.with_conn(|conn| {
        conn.execute("DELETE FROM ticket_current_worker_assignments WHERE workspace_id='w' AND assignment_id='cleared'",[])?; Ok(())
    }).unwrap();
    f.assign(&current, "historical", Some("done"), false);
    f.assign(&current, "current", Some("waiting"), true);
    let all = f.run(Default::default());
    assert_eq!(all.items.len(), 4);
    for id in [&unassigned, &cleared] {
        assert!(
            all.items
                .iter()
                .find(|item| &item.id == id)
                .unwrap()
                .assignment_tracking
                .is_empty()
        );
    }
    assert!(
        all.items
            .iter()
            .find(|item| item.id == unknown)
            .unwrap()
            .assignment_tracking[0]
            .tracking
            .is_none()
    );
    let unknown_projection = &all
        .items
        .iter()
        .find(|item| item.id == unknown)
        .unwrap()
        .assignment_tracking;
    assert_eq!(
        unknown_projection[0].last_event_id.as_deref(),
        Some("legacy-assign")
    );
    assert_eq!(
        *unknown_projection,
        f.authority.ticket_assignment_tracking(&unknown).unwrap()
    );
    let mut assigned = ids(&f.run(TicketBrowseQueryRequest {
        assigned_principal: Some(assigned_subject()),
        ..Default::default()
    }));
    assigned.sort();
    let mut expected = vec![current.clone(), unknown];
    expected.sort();
    assert_eq!(assigned, expected);
    assert!(
        f.run(TicketBrowseQueryRequest {
            handling_status: Some(ticket::TicketAssignmentTrackingStatus::Done),
            ..Default::default()
        })
        .items
        .is_empty()
    );
    assert_eq!(
        ids(&f.run(TicketBrowseQueryRequest {
            handling_status: Some(ticket::TicketAssignmentTrackingStatus::Waiting),
            ..Default::default()
        })),
        vec![current]
    );
    for principal in [
        ticket::TicketAssignmentPrincipalSnapshot::User {
            account_id: "missing".into(),
        },
        ticket::TicketAssignmentPrincipalSnapshot::Worker {
            runtime_id: "missing".into(),
            worker_id: "missing".into(),
        },
    ] {
        assert!(
            f.run(TicketBrowseQueryRequest {
                assigned_principal: Some(principal),
                ..Default::default()
            })
            .items
            .is_empty()
        );
    }
}

#[tokio::test]
async fn ticket_browse_search_literal_title_initial_thread_and_closure_history() {
    let f = Fixture::new().await;
    let title = f.ticket("Literal %_ title", Some("1"));
    let body = f.ticket("body", Some("1"));
    f.event(&body, -1, "Initial %_ BODY", false);
    let thread = f.ticket("thread", Some("1"));
    f.event(&thread, 80, "Thread %_ needle", false);
    let historical = f.ticket("historical", Some("1"));
    f.closure(
        &historical,
        "old",
        TicketClosureReason::Other("%_ custom".into()),
        true,
    );
    f.closure(&historical, "new", TicketClosureReason::Completed, false);
    let archived = f.ticket("archived", Some("1"));
    f.event(&archived, 90, "Archived %_", true);
    f.ticket("anything", Some("1"));
    let mut expected = vec![title, body, thread, historical.clone()];
    expected.sort();
    let result = f.run(TicketBrowseQueryRequest {
        query: Some("%_".into()),
        ..Default::default()
    });
    assert_eq!(ids(&result), expected);
    assert!(
        result
            .items
            .iter()
            .find(|item| item.id == historical)
            .unwrap()
            .current_closure
            .is_none()
    );
    assert_eq!(
        ids(&f.run(TicketBrowseQueryRequest {
            query: Some("other %_ custom".into()),
            ..Default::default()
        })),
        vec![historical]
    );
    assert!(
        f.run(TicketBrowseQueryRequest {
            closure_reason: Some(TicketClosureReason::Other("%_ custom".into())),
            ..Default::default()
        })
        .items
        .is_empty()
    );
    assert!(
        f.run(TicketBrowseQueryRequest {
            query: Some("archived %_".into()),
            ..Default::default()
        })
        .items
        .is_empty()
    );
}

#[tokio::test]
async fn ticket_browse_current_closure_not_prior_or_reopened_and_workspace_isolation() {
    let f = Fixture::new().await;
    let id = f.ticket("same identity", Some("1"));
    f.closure(&id, "previous", TicketClosureReason::Completed, true);
    f.closure(&id, "current", TicketClosureReason::NotPlanned, true);
    assert!(
        f.run(TicketBrowseQueryRequest {
            closure_reason: Some(TicketClosureReason::Completed),
            ..Default::default()
        })
        .items
        .is_empty()
    );
    assert_eq!(
        ids(&f.run(TicketBrowseQueryRequest {
            closure_reason: Some(TicketClosureReason::NotPlanned),
            ..Default::default()
        })),
        vec![id.clone()]
    );
    f.authority.store.with_conn(|conn| {
        let columns: Vec<String> = conn.prepare("PRAGMA table_info(typed_tickets)")?.query_map([],|row| row.get(1))?.collect::<rusqlite::Result<_>>()?;
        let expressions = columns.iter().map(|name| match name.as_str() { "workspace_id" => "'foreign'".into(), "title" => "'foreign secret'".into(), _ => format!("\"{name}\"") }).collect::<Vec<_>>().join(",");
        conn.execute(&format!("INSERT INTO typed_tickets SELECT {expressions} FROM typed_tickets WHERE workspace_id='w' AND ticket_id=?1"),[&id])?;
        conn.execute("INSERT INTO workspace_resource_keys(workspace_id,resource_kind,resource_id,sequence,resource_key,allocated_at) VALUES('foreign','ticket',?1,1,'T-1','1')",[&id])?;
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body,event_ref,archived) VALUES('foreign',?1,-1,'comment','foreign secret','foreign-event',0)",[&id])?;
        conn.execute("INSERT INTO workspace_tags(workspace_id,tag_id,name,normalized_name,last_operation_key,deleted) VALUES('foreign','foreign-tag','Foreign','foreign','tag-op',0)",[])?;
        conn.execute("INSERT INTO typed_ticket_tags(workspace_id,ticket_id,tag_id) VALUES('foreign',?1,'foreign-tag')",[&id])?;
        Ok(())
    }).unwrap();
    let local = f.run(Default::default());
    assert_eq!(local.items.len(), 1);
    assert!(local.items[0].tags.is_empty());
    assert!(
        f.run(TicketBrowseQueryRequest {
            query: Some("foreign secret".into()),
            ..Default::default()
        })
        .items
        .is_empty()
    );
    assert!(
        f.run(TicketBrowseQueryRequest {
            tag_ids: vec!["foreign-tag".into()],
            ..Default::default()
        })
        .items
        .is_empty()
    );
    let foreign =
        SqliteWorkspaceAuthority::new(f._dir.path().join("browse.db"), "foreign").unwrap();
    let foreign = foreign.query_ticket_browse(Default::default()).unwrap();
    assert_eq!(foreign.items[0].title, "foreign secret");
    assert!(foreign.items[0].current_closure.is_none());
    assert_eq!(foreign.items[0].tags[0].id, "foreign-tag");
}

#[tokio::test]
async fn ticket_browse_user_worker_principals_are_exact_current_identity_not_handling() {
    let f = Fixture::new().await;
    let id = f.ticket("current roles", Some("1"));
    f.assign(&id, "orchestrator", Some("waiting"), true);
    f.authority.store.with_conn(|conn| {
        conn.execute("INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at) VALUES('w','runtime','worker','Fixture','normal','1','1')",[])?;
        conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w',?1,'owner','owner','user','owner-account','owner-account','1')",[&id])?;
        conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w',?1,'owner','owner','user','owner-account','1')",[&id])?;
        conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,runtime_id,worker_id,assigned_by,assigned_at) VALUES('w',?1,'worker','worker','worker','runtime','worker','owner-account','1')",[&id])?;
        conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,runtime_id,worker_id,updated_at) VALUES('w',?1,'worker','worker','worker','runtime','worker','1')",[&id])?;
        Ok(())
    }).unwrap();
    for principal in [
        ticket::TicketAssignmentPrincipalSnapshot::User {
            account_id: "owner-account".into(),
        },
        ticket::TicketAssignmentPrincipalSnapshot::Worker {
            runtime_id: "runtime".into(),
            worker_id: "worker".into(),
        },
    ] {
        let query = TicketBrowseQueryRequest {
            assigned_principal: Some(principal),
            ..Default::default()
        };
        let result = f.run(query.clone());
        assert_eq!(ids(&result), vec![id.clone()]);
        assert_eq!(result.items[0].assignment_tracking.len(), 1);
        assert!(
            f.run(TicketBrowseQueryRequest {
                handling_status: Some(ticket::TicketAssignmentTrackingStatus::Waiting),
                ..query
            })
            .items
            .is_empty()
        );
    }
    assert!(
        f.run(TicketBrowseQueryRequest {
            assigned_principal: Some(ticket::TicketAssignmentPrincipalSnapshot::Worker {
                runtime_id: "different-runtime".into(),
                worker_id: "worker".into()
            }),
            ..Default::default()
        })
        .items
        .is_empty()
    );
}

#[tokio::test]
async fn ticket_browse_plain_page_sql_uses_recent_index_without_history_or_graph() {
    let f = Fixture::new().await;
    f.ticket("first", Some("1"));
    let (sql, values) = page_sql("w", &TicketBrowseQueryRequest::default(), None, 30);
    for forbidden in [
        "typed_ticket_events",
        "typed_ticket_case_history",
        "typed_ticket_relations",
        "merge_request",
        "tracking_history",
    ] {
        assert!(!sql.contains(forbidden), "{sql}");
    }
    f.authority
        .store
        .with_conn(|conn| {
            let plans: Vec<String> = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
                .query_map(params_from_iter(values.iter()), |row| row.get(3))?
                .collect::<rusqlite::Result<_>>()?;
            assert!(
                plans
                    .iter()
                    .any(|plan| plan.contains("typed_tickets_workspace_updated")),
                "{plans:?}"
            );
            assert!(
                !plans.iter().any(|plan| plan.contains("TEMP B-TREE")),
                "{plans:?}"
            );
            Ok(())
        })
        .unwrap();
}
