// Host/domain integration through real SQLite and public migration entrypoints.
// Retained fixtures use pre-posts DDL; no current schema is relabelled as old.
fn ticket_posts_schema93_fixture(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    configure_sqlite(&conn).unwrap();
    conn.execute_batch(include_str!("frozen_ticket_schema_v10.sql"))
        .unwrap();
    conn.execute_batch("CREATE TABLE ticket_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL);
        INSERT INTO ticket_schema_migrations VALUES(10,'ticket schema baseline','original-component10-time');
        PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;").unwrap();
    create_workspace_schema_structure_from_sql(&conn, include_str!("frozen_schema_v93.sql"), false)
        .unwrap();
    conn.execute_batch(include_str!(
        "../../ticket/src/assignment_tracking_schema.sql"
    ))
    .unwrap();
    conn.execute_batch("INSERT INTO ticket_schema_migrations VALUES(11,'Ticket-owned assignment tracking','original-component11-time');
        INSERT INTO __yoi_schema_migrations(version,name) VALUES(93,'workspace schema baseline');
        PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;").unwrap();
    assignment_delivery::install(&conn).unwrap();
    seed_migration_workspace(&conn);
    conn
}

fn seed_ticket_posts_legacy_content(conn: &Connection, workspace: &str) {
    for (id, body) in [
        ("authored", "Current requirements after edits"),
        ("unknown", "Unknown creator body"),
    ] {
        conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,created_at,updated_at,workflow_state,workflow_state_explicit)
            VALUES(?1,?2,?2,'Retained Ticket','open','task','P2',?3,'2026-01-01T00:00:00Z','2026-02-01T00:00:00Z','open',1)", params![workspace,id,body]).unwrap();
    }
    for (index, kind, author, body) in [
        (0, "create", Some("creator"), "Original creation audit"),
        (3, "comment", Some("commenter"), "A preserved comment"),
        (7, "item_edit", Some("editor"), "Original edit audit"),
        (9, "comment", None, "Anonymous historical comment"),
    ] {
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,body) VALUES(?1,'authored',?2,?3,?4,'2026-01-02T00:00:00Z',?5)",params![workspace,index,kind,author,body]).unwrap();
        conn.execute("INSERT INTO typed_ticket_event_attributes VALUES(?1,'authored',?2,'source_kind','original-source')",params![workspace,index]).unwrap();
        conn.execute("INSERT INTO typed_ticket_event_references VALUES(?1,'authored',?2,0,'ticket','T-original')",params![workspace,index]).unwrap();
    }
}

#[test]
fn ticket_posts_schema93_preserves_initial_body_creator_comments_and_archived_audit() {
    use ticket::TicketBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let conn = ticket_posts_schema93_fixture(&path);
    seed_ticket_posts_legacy_content(&conn, "space");
    let old_events = stored_fixture_rows(&conn, "typed_ticket_events");
    let old_attributes = stored_fixture_rows(&conn, "typed_ticket_event_attributes");
    let old_references = stored_fixture_rows(&conn, "typed_ticket_event_references");
    let old_markers = stored_fixture_rows(&conn, "ticket_schema_migrations");
    drop(conn);
    let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
    assert_eq!(plan.current_schema_version, 93);
    assert_eq!(
        plan.migrations
            .iter()
            .map(|step| step.version)
            .collect::<Vec<_>>(),
        (94..=LATEST_SCHEMA_VERSION).collect::<Vec<_>>()
    );
    let conn = Connection::open(&path).unwrap();
    assert!(
        column_exists(&conn, "typed_tickets", "body").unwrap(),
        "planning must not mutate its source"
    );
    assert_eq!(
        stored_fixture_rows(&conn, "typed_ticket_events"),
        old_events
    );
    drop(conn);
    assert_eq!(SqliteWorkspaceStore::migrate_database(&path).unwrap(), plan);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store.with_conn(|conn| {
        assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
        assert!(!column_exists(conn,"typed_tickets","body")?);
        assert_eq!(stored_fixture_rows(conn,"typed_ticket_events"),old_events);
        assert_eq!(stored_fixture_rows(conn,"typed_ticket_event_attributes"),old_attributes);
        assert_eq!(stored_fixture_rows(conn,"typed_ticket_event_references"),old_references);
        let markers = stored_fixture_rows(conn,"ticket_schema_migrations");
        assert_eq!(&markers[..old_markers.len()],old_markers.as_slice());
        assert_eq!(markers.len(),old_markers.len()+4);
        let new_markers: Vec<(i64,String)> = conn.prepare("SELECT version,name FROM ticket_schema_migrations WHERE version>11 ORDER BY version")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        assert_eq!(new_markers,vec![
            (12,"Canonical Ticket posts".into()),
            (13,"Ticket content change facts".into()),
            (14,"Subject Ticket responsibility".into()),
            (15,"Ticket Service transactional outbox".into()),
        ]);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM typed_ticket_service_outbox",[],|r|r.get::<_,i64>(0))?,0,"migration does not invent Service delivery facts");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM typed_ticket_content_changes",[],|r|r.get::<_,i64>(0))?,0,"migration does not invent content change facts");
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM ticket_assignment_content_inputs",[],|r|r.get::<_,i64>(0))?,0,"migration does not turn historical edits into new inputs");
        let initial: (String,Option<String>,Option<String>,String,String) = conn.query_row("SELECT e.body,t.created_by,e.author,e.kind,e.event_ref FROM typed_tickets t JOIN typed_ticket_events e USING(workspace_id,ticket_id) WHERE t.ticket_id='authored' AND e.event_index=-1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)))?;
        assert_eq!(initial,("Current requirements after edits".into(),Some("creator".into()),Some("creator".into()),"comment".into(),"authored:-1".into()));
        let archive: Vec<(i64,String,i64)> = conn.prepare("SELECT event_index,event_ref,archived FROM typed_ticket_events WHERE ticket_id='authored' ORDER BY event_index")?.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
        assert_eq!(archive,vec![(-1,"authored:-1".into(),0),(0,"authored:0".into(),1),(3,"authored:3".into(),0),(7,"authored:7".into(),1),(9,"authored:9".into(),0)]);
        let unknown: (Option<String>,Option<String>,String) = conn.query_row("SELECT t.created_by,e.author,e.body FROM typed_tickets t JOIN typed_ticket_events e USING(workspace_id,ticket_id) WHERE t.ticket_id='unknown' AND e.event_index=-1",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        assert_eq!(unknown,(None,None,"Unknown creator body".into()));
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0))?,0);
        ticket::verify_sqlite_ticket_schema(conn).unwrap();
        Ok(())
    }).unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap();
    let loaded = backend.show("authored".into()).unwrap();
    assert_eq!(loaded.meta.created_by.as_deref(), Some("creator"));
    assert_eq!(
        loaded.document.body.as_str(),
        "Current requirements after edits"
    );
    assert_eq!(
        loaded
            .events
            .iter()
            .map(|event| event.event_ref.as_str())
            .collect::<Vec<_>>(),
        ["authored:-1", "authored:3", "authored:9"]
    );
    assert!(loaded.events[0].is_initial);
    assert_eq!(loaded.events[1].author.as_deref(), Some("commenter"));
    assert_eq!(loaded.events[2].author, None);
    assert!(
        loaded
            .events
            .iter()
            .all(|event| event.edited_at.is_none() && event.edited_by.is_none())
    );
    drop(store);
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    reopened
        .with_conn(|conn| {
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM typed_ticket_events WHERE event_index=-1",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                2
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM ticket_schema_migrations WHERE version=12",
                    [],
                    |r| r.get::<_, i64>(0)
                )?,
                1
            );
            assert_eq!(
                conn.query_row(
                    "SELECT name FROM ticket_schema_migrations WHERE version=15",
                    [],
                    |r| r.get::<_, String>(0)
                )?,
                "Ticket Service transactional outbox"
            );
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM typed_ticket_service_outbox", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn ticket_posts_schema94_keeps_body_edit_metadata_and_published_refs_across_reopen() {
    use ticket::TicketBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let conn = ticket_posts_schema93_fixture(&path);
    seed_ticket_posts_legacy_content(&conn, "space");
    conn.execute_batch("INSERT INTO typed_ticket_event_attributes VALUES
        ('space','authored',7,'changes','title,body'),
        ('space','authored',3,'event_id','raw-identity'),
        ('space','authored',9,'event_id','raw-identity');
        INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,body)
        VALUES('space','authored',15,'item_edit','title-editor','2026-03-01T00:00:00Z','Later title edit');
        INSERT INTO typed_ticket_event_attributes VALUES('space','authored',15,'changes','title');").unwrap();
    drop(conn);
    SqliteWorkspaceStore::migrate_database(&path).unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap();
    let migrated = backend.show("authored".into()).unwrap();
    assert_eq!(migrated.meta.created_by.as_deref(), Some("creator"));
    assert_eq!(
        migrated.meta.created_at.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );
    let initial = &migrated.events[0];
    assert_eq!(initial.author.as_deref(), Some("creator"));
    assert_eq!(initial.at.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert_eq!(initial.edited_by.as_deref(), Some("editor"));
    assert_eq!(initial.edited_at.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert_eq!(
        migrated
            .events
            .iter()
            .map(|e| e.event_ref.as_str())
            .collect::<Vec<_>>(),
        ["authored:-1", "authored:3", "authored:9"]
    );
    assert_eq!(migrated.events[1].attributes["event_id"], "authored:3");
    assert_eq!(migrated.events[2].attributes["event_sequence"], "9");
    let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    reopened.with_conn(|conn| {
        assert_eq!(conn.query_row("SELECT value FROM typed_ticket_event_attributes WHERE ticket_id='authored' AND event_index=3 AND key='event_id'", [], |r| r.get::<_,String>(0))?, "raw-identity");
        assert_eq!(conn.query_row("SELECT archived FROM typed_ticket_events WHERE ticket_id='authored' AND event_index=15", [], |r| r.get::<_,i64>(0))?, 1);
        Ok(())
    }).unwrap();
    assert_eq!(backend.show("authored".into()).unwrap(), migrated);
    backend
        .add_event(
            "authored".into(),
            ticket::NewTicketEvent::new(ticket::TicketEventKind::Comment, "Next discussion"),
        )
        .unwrap();
    assert_eq!(
        backend
            .show("authored".into())
            .unwrap()
            .events
            .last()
            .unwrap()
            .event_ref,
        "authored:16"
    );
}

#[test]
fn ticket_posts_schema94_host_failure_rolls_back_domain_rows_ddl_and_markers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    let conn = ticket_posts_schema93_fixture(&path);
    seed_ticket_posts_legacy_content(&conn, "space");
    conn.execute_batch("CREATE TRIGGER reject_host94 BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=94 BEGIN SELECT RAISE(ABORT,'injected host94 failure'); END;").unwrap();
    let schema = fixture_query_rows(
        &conn,
        "SELECT type,name,sql FROM sqlite_schema ORDER BY type,name",
    );
    let tables = [
        "typed_tickets",
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_event_references",
        "ticket_schema_migrations",
        "__yoi_schema_migrations",
    ];
    let rows = tables
        .map(|table| fixture_query_rows(&conn, &format!("SELECT * FROM {table} ORDER BY rowid")));
    drop(conn);
    for entrypoint in 0..3 {
        let error = match entrypoint {
            0 => SqliteWorkspaceStore::open_for_test(&path).err().unwrap(),
            1 => SqliteWorkspaceStore::migration_plan(&path).unwrap_err(),
            _ => SqliteWorkspaceStore::migrate_database(&path).unwrap_err(),
        };
        assert!(
            error.to_string().contains("injected host94 failure"),
            "{error}"
        );
        let conn = Connection::open(&path).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 93);
        assert_eq!(
            fixture_query_rows(
                &conn,
                "SELECT type,name,sql FROM sqlite_schema ORDER BY type,name"
            ),
            schema
        );
        for (table, before) in tables.iter().zip(&rows) {
            assert_eq!(
                &fixture_query_rows(&conn, &format!("SELECT * FROM {table} ORDER BY rowid")),
                before,
                "rollback {table}"
            );
        }
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TRIGGER reject_host94;").unwrap();
    drop(conn);
    SqliteWorkspaceStore::open_for_test(&path).unwrap();
}

#[test]
fn ticket_posts_fresh_and_migrated_schema_constraints_match() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    drop(ticket_posts_schema93_fixture(&path));
    let upgraded = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    for table in [
        "typed_tickets",
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_event_references",
        "ticket_assignment_content_inputs",
    ] {
        for query in [
            format!(
                "SELECT name,type,\"notnull\",dflt_value,pk FROM pragma_table_info('{table}') ORDER BY name"
            ),
            format!(
                "SELECT \"table\",\"from\",\"to\",on_update,on_delete FROM pragma_foreign_key_list('{table}') ORDER BY 1,2,3"
            ),
            format!(
                "SELECT i.\"unique\",i.partial,(SELECT group_concat(name, ',') FROM (SELECT name FROM pragma_index_info(i.name) ORDER BY seqno)) FROM pragma_index_list('{table}') i ORDER BY 1,2,3"
            ),
            format!(
                "SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND tbl_name='{table}' ORDER BY name"
            ),
        ] {
            let expected = fresh
                .with_conn(|conn| Ok(fixture_query_rows(conn, &query)))
                .unwrap();
            let actual = upgraded
                .with_conn(|conn| Ok(fixture_query_rows(conn, &query)))
                .unwrap();
            assert_eq!(actual, expected, "{query}");
        }
    }
    for store in [&fresh, &upgraded] {
        store.with_conn(|conn| {
            seed_migration_workspace_if_missing(conn);
            conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES('space','guard','guard','Guard','open','task','P2','open',1)",[])?;
            conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('space','guard',-1,'guard:-1','comment','Initial')",[])?;
            assert!(conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('space','guard',0,'guard:-1','comment','Duplicate ref')",[]).is_err());
            assert!(conn.execute("UPDATE typed_ticket_events SET archived=2 WHERE ticket_id='guard'",[]).is_err());
            assert!(conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('space','missing',0,'missing:0','comment','Orphan')",[]).is_err());
            Ok(())
        }).unwrap();
    }
}

fn seed_migration_workspace_if_missing(conn: &Connection) {
    if conn
        .query_row(
            "SELECT COUNT(*) FROM workspaces WHERE workspace_id='space'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 0
    {
        seed_migration_workspace(conn);
    }
}

#[test]
fn ticket_posts_oldest_supported_host50_preserves_content_through_full_chain() {
    use ticket::TicketBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace.db");
    prepare_schema_v50(&path, Some("workspace-a"));
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_ticket_posts_legacy_content(&conn, "workspace-a");
    conn.execute("UPDATE typed_tickets SET workflow_state='planning'", [])
        .unwrap();
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            ticket::verify_sqlite_ticket_schema(conn).unwrap();
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                    .get::<_, i64>(
                    0
                ))?,
                0
            );
            Ok(())
        })
        .unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "workspace-a").unwrap();
    let loaded = backend.show("authored".into()).unwrap();
    assert_eq!(
        loaded.document.body.as_str(),
        "Current requirements after edits"
    );
    assert_eq!(loaded.meta.created_by.as_deref(), Some("creator"));
    assert_eq!(
        loaded
            .events
            .iter()
            .map(|event| event.event_ref.as_str())
            .collect::<Vec<_>>(),
        ["authored:-1", "authored:3", "authored:9"]
    );
}
