//! Reproducible component benchmark: disposable SQLite, real authority methods, no HTTP/Runtime.
//! Run with `cargo test -p yoi-workspace-server ticket_browse_component_performance --lib -- --ignored --nocapture`.
use crate::authority::{SqliteWorkspaceAuthority, TicketAuthority};
use crate::store::{ControlPlaneStore, SqliteWorkspaceStore, WorkspaceRecord};
use rusqlite::{Connection, params};
use std::time::Instant;
use ticket::{SqliteTicketBackend, TicketBackend};

async fn fixture(
    tickets: usize,
    events_per_ticket: usize,
) -> (tempfile::TempDir, SqliteWorkspaceAuthority) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("benchmark.db");
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: "bench".into(),
            owner_account_id: "owner-account".into(),
            display_name: "Component benchmark".into(),
            state: "active".into(),
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
        })
        .await
        .unwrap();
    let backend = SqliteTicketBackend::open_verified(&path, "bench").unwrap();
    let seed = backend
        .create(ticket::NewTicket::new("Benchmark seed"))
        .unwrap();
    let mut conn = Connection::open(&path).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    let ticket_columns: Vec<String> = conn
        .prepare("PRAGMA table_info(typed_tickets)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let expressions = ticket_columns
        .iter()
        .map(|name| match name.as_str() {
            "ticket_id" | "slug" => "?1".to_owned(),
            "title" => "?2".to_owned(),
            "created_at" | "updated_at" => "'2026-10-01T00:00:00Z'".to_owned(),
            _ => format!("\"{name}\""),
        })
        .collect::<Vec<_>>()
        .join(",");
    let clone_ticket = format!(
        "INSERT INTO typed_tickets SELECT {expressions} FROM typed_tickets WHERE workspace_id='bench' AND ticket_id=?3"
    );
    let tx = conn.transaction().unwrap();
    {
        let mut insert_ticket = tx.prepare(&clone_ticket).unwrap();
        let mut insert_key = tx.prepare("INSERT INTO workspace_resource_keys(workspace_id,resource_kind,resource_id,sequence,resource_key,allocated_at) VALUES('bench','ticket',?1,?2,?3,'2026-10-01T00:00:00Z')").unwrap();
        let mut insert_event = tx.prepare("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body,event_ref,archived) VALUES('bench',?1,?2,'comment',?3,?4,0)").unwrap();
        let mut insert_attribute = tx.prepare("INSERT INTO typed_ticket_event_attributes(workspace_id,ticket_id,event_index,key,value) VALUES('bench',?1,?2,'bench','value')").unwrap();
        let mut insert_reference = tx.prepare("INSERT INTO typed_ticket_event_references(workspace_id,ticket_id,event_index,ordinal,kind,target) VALUES('bench',?1,?2,0,'ticket',?3)").unwrap();
        let mut insert_relation = tx.prepare("INSERT INTO typed_ticket_relations(workspace_id,ticket_id,kind,target,author,at) VALUES('bench',?1,'related',?2,'owner-account','2026-10-01T00:00:00Z')").unwrap();
        let body = "component benchmark text ".repeat(40);
        for i in 0..tickets {
            let id = format!("bench-{i:06}");
            insert_ticket
                .execute(params![id, format!("Ticket {i:06}"), seed.id])
                .unwrap();
            insert_key
                .execute(params![id, i + 2, format!("T-{}", i + 2)])
                .unwrap();
            insert_relation.execute(params![id, seed.id]).unwrap();
            for e in -1..events_per_ticket as i64 {
                let text = if e == events_per_ticket as i64 - 1 {
                    format!("search-needle {body}")
                } else {
                    body.clone()
                };
                insert_event
                    .execute(params![id, e, text, format!("{id}-event-{e}")])
                    .unwrap();
                insert_attribute.execute(params![id, e]).unwrap();
                insert_reference.execute(params![id, e, seed.id]).unwrap();
            }
        }
    }
    tx.commit().unwrap();
    drop(conn);
    let authority = SqliteWorkspaceAuthority::new(path, "bench").unwrap();
    (dir, authority)
}

fn query(text: Option<&str>) -> server_api::TicketQueryRequest {
    server_api::TicketQueryRequest {
        states: vec![ticket::TicketState::Open],
        sort: Some("updated_desc".into()),
        limit: Some(30),
        query: text.map(str::to_owned),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "component timing benchmark; no production services"]
async fn ticket_browse_component_performance() {
    let tickets = std::env::var("BROWSE_BENCH_TICKETS")
        .ok()
        .map(|v| v.parse().unwrap())
        .unwrap_or(1_000);
    let events = std::env::var("BROWSE_BENCH_EVENTS")
        .ok()
        .map(|v| v.parse().unwrap())
        .unwrap_or(100);
    let (_dir, authority) = fixture(tickets, events).await;
    for text in [
        None,
        Some("search-needle"),
        Some("Ticket 00099"),
        Some("absent-needle"),
    ] {
        let request = query(text);
        let browse = server_api::TicketBrowseQueryRequest {
            states: request.states.clone(),
            sort: request.sort.clone(),
            limit: request.limit,
            query: request.query.clone(),
            ..Default::default()
        };
        // Warm both paths on the same database; exclude fixture generation.
        authority.query_tickets(request.clone()).unwrap();
        authority.query_ticket_browse(browse.clone()).unwrap();
        let mut old_times = Vec::new();
        let mut new_times = Vec::new();
        let mut old_bytes = 0;
        let mut new_bytes = 0;
        // Alternate paths to reduce bias from unrelated machine activity. Include
        // complete response JSON serialization, but not parity assertions, in timings.
        for _ in 0..3 {
            let started = Instant::now();
            let old = authority.query_tickets(request.clone()).unwrap();
            old_bytes = serde_json::to_vec(&old).unwrap().len();
            old_times.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let new = authority.query_ticket_browse(browse.clone()).unwrap();
            new_bytes = serde_json::to_vec(&new).unwrap().len();
            new_times.push(started.elapsed().as_secs_f64() * 1000.0);
            let projected = old
                .items
                .into_iter()
                .map(|item| server_api::TicketBrowseQueryItem {
                    id: item.id,
                    resource_key: item.resource_key,
                    title: item.title,
                    state: item.state,
                    updated_at: item.updated_at,
                    tags: item.tags,
                    current_closure: item.current_closure,
                    assignment_tracking: item.assignment_tracking,
                })
                .collect::<Vec<_>>();
            assert_eq!(projected, new.items);
            assert!(new.items.len() <= 30);
            if text == Some("absent-needle") {
                assert!(new.items.is_empty());
            }
            assert_eq!(old.page.has_more, new.page.has_more);
        }
        old_times.sort_by(f64::total_cmp);
        new_times.sort_by(f64::total_cmp);
        println!(
            "COMPARE tickets={tickets} events_each={events} search={text:?} samples=3 old_ms={old_times:?} new_ms={new_times:?} median_speedup={:.1} old_bytes={old_bytes} new_bytes={new_bytes}",
            old_times[1] / new_times[1]
        );
    }
}
