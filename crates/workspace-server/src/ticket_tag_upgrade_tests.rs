// Included by store::tests. These fixtures prove composition with the real
// current Workspace schema-90 startup path, not a live user's database.

fn classification_rows(conn: &Connection) -> std::collections::BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    ["workspace_tags", "typed_ticket_tags", "workspace_tag_operations",
     "typed_ticket_events", "typed_ticket_event_attributes", "typed_ticket_labels"]
        .into_iter()
        .map(|table| {
            let mut statement = conn.prepare(&format!("SELECT * FROM {table} WHERE workspace_id='space' ORDER BY rowid")).unwrap();
            let columns = statement.column_count();
            let rows = statement.query_map([], |row| {
                (0..columns).map(|column| row.get(column)).collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
            }).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            (table.to_owned(), rows)
        }).collect()
}

#[test]
fn current_workspace90_without_classification_upgrades_ticket7_without_rewriting_ticket_history() {
    use ticket::TicketBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let store = SqliteWorkspaceStore::open(&path).unwrap();
    store.with_conn(|conn| { seed_migration_workspace(conn); Ok(()) }).unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap();
    let id = backend.create(ticket::NewTicket::new("Existing closed Ticket")).unwrap().id;
    backend.close(id.clone().into(), ticket::MarkdownText::new("Existing decision, not a classification")).unwrap();
    store.with_conn(|conn| {
        conn.execute("INSERT INTO typed_ticket_labels VALUES ('space', ?1, 0, 'ready')", [&id])?;
        Ok(())
    }).unwrap();
    let before = backend.show(id.clone().into()).unwrap();
    assert!(before.tags.is_empty());
    // Current develop has Workspace schema 90 and pre-classification Ticket
    // schema 7. Strip only the not-yet-installed component from a fresh current
    // fixture, leaving every Workspace FK, trigger, archive and Ticket row intact.
    store.with_conn(|conn| {
        assert_eq!(current_schema_version(conn)?, 90);
        conn.execute_batch("DROP TABLE typed_ticket_tags; DROP TABLE workspace_tag_operations; DROP TABLE workspace_tags;
            DELETE FROM ticket_schema_migrations;
            INSERT INTO ticket_schema_migrations VALUES (7, 'ticket schema baseline', 'before-tags');")?;
        ticket::verify_pre_classification_ticket_schema(conn).unwrap();
        Ok(())
    }).unwrap();
    drop(backend);
    drop(store);
    let upgraded = SqliteWorkspaceStore::open(&path).unwrap();
    upgraded.with_conn(|conn| {
        assert_eq!(current_schema_version(conn)?, 90);
        assert_eq!(conn.query_row("SELECT version FROM ticket_schema_migrations", [], |row| row.get::<_, i64>(0))?, 8);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get::<_, i64>(0))?, 0);
        assert!(!column_exists(conn, "workspace_runtime_bindings", "state")?);
        assert!(table_exists(conn, "workspace_runtime_verification_archive")?);
        ticket::verify_sqlite_ticket_schema(conn).unwrap();
        Ok(())
    }).unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap();
    assert_eq!(backend.show(id.clone().into()).unwrap(), before);
    let ready = backend.mutate_workspace_tag(ticket::WorkspaceTagMutation { operation_key:"create-ready".into(), action:ticket::WorkspaceTagAction::Create { name:"ready".into() } }).unwrap();
    backend.set_ticket_tags(id.clone().into(), ticket::TicketTagSetRequest { operation_key:"attach-ready".into(), expected_tag_ids:vec![], tag_ids:vec![ready.id] }).unwrap();
    let after = backend.show(id.into()).unwrap();
    assert_eq!(after.meta, before.meta);
    assert_eq!(after.resolution, before.resolution);
    assert_eq!(ticket::ticket_content_digest(&after), ticket::ticket_content_digest(&before));
}

#[test]
fn workspace89_to90_runtime_rebuild_preserves_tag_links_tombstones_receipts_and_audit() {
    use ticket::TicketBackend;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    prepare_retained_schema(&path, 89);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    seed_migration_workspace(&conn);
    ticket::migrate_sqlite_ticket_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO workspace_runtime_bindings
        (workspace_id,runtime_id,display_name,base_url,public_key,public_key_fingerprint,binding_id,state,authentication_mode,workspace_key_id,workspace_public_key_fingerprint,workspace_trust_id,created_at,updated_at,revoked_at)
        VALUES ('space','runtime','Runtime','https://runtime.invalid','public','fingerprint','binding','verified','workspace_identity','key','workspace-fingerprint','trust','created','updated',NULL);") .unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap()
        .with_event_attributes(std::collections::BTreeMap::from([("source_account_id".into(), "owner".into())]));
    let id = backend.create(ticket::NewTicket::new("Classified before auth migration")).unwrap().id;
    let ready = backend.mutate_workspace_tag(ticket::WorkspaceTagMutation {operation_key:"ready".into(),action:ticket::WorkspaceTagAction::Create{name:"ready".into()}}).unwrap();
    let old = backend.mutate_workspace_tag(ticket::WorkspaceTagMutation {operation_key:"bug".into(),action:ticket::WorkspaceTagAction::Create{name:"Bug".into()}}).unwrap();
    let attach = ticket::TicketTagSetRequest {operation_key:"attach".into(),expected_tag_ids:vec![],tag_ids:vec![ready.id.clone(),old.id.clone()]};
    let receipt = backend.set_ticket_tags(id.clone().into(), attach.clone()).unwrap();
    let deleted = backend.mutate_workspace_tag(ticket::WorkspaceTagMutation {operation_key:"delete".into(),action:ticket::WorkspaceTagAction::Delete{tag_id:old.id.clone(),expected_last_operation_key:old.last_operation_key}}).unwrap();
    assert!(deleted.deleted);
    let replacement = backend.mutate_workspace_tag(ticket::WorkspaceTagMutation {operation_key:"replacement".into(),action:ticket::WorkspaceTagAction::Create{name:"Bug".into()}}).unwrap();
    assert_ne!(replacement.id, old.id);
    let before = backend.show(id.clone().into()).unwrap();
    let snapshots = classification_rows(&conn);
    drop(conn);
    drop(backend);
    let upgraded = SqliteWorkspaceStore::open(&path).unwrap();
    upgraded.with_conn(|conn| {
        assert_eq!(current_schema_version(conn)?, 90);
        assert_eq!(classification_rows(conn), snapshots);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get::<_, i64>(0))?, 0);
        assert_eq!(conn.query_row("SELECT state FROM workspace_runtime_binding_verification_archive WHERE workspace_id='space' AND runtime_id='runtime'", [], |row| row.get::<_, String>(0))?, "verified");
        assert!(!column_exists(conn, "workspace_runtime_bindings", "state")?);
        ticket::verify_sqlite_ticket_schema(conn).unwrap();
        // Composite FKs still reject foreign Ticket/tag linkage after rebuild.
        assert!(conn.execute("INSERT INTO typed_ticket_tags VALUES ('other','missing',?1)", [&ready.id]).is_err());
        Ok(())
    }).unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "space").unwrap()
        .with_event_attributes(std::collections::BTreeMap::from([("source_account_id".into(), "owner".into())]));
    assert_eq!(backend.show(id.clone().into()).unwrap(), before);
    assert_eq!(backend.set_ticket_tags(id.clone().into(), attach).unwrap(), receipt);
    assert!(backend.set_ticket_tags(id.into(), ticket::TicketTagSetRequest {operation_key:"deleted-reference".into(),expected_tag_ids:vec![ready.id],tag_ids:vec![old.id]}).is_err());
    upgraded.with_conn(|conn| { assert_eq!(classification_rows(conn), snapshots); Ok(()) }).unwrap();
}
