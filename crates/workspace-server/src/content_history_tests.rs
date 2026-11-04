//! Contract fixture for the actual Server database/transaction owner and the content adapter.
//! Existing authorization is represented by an exact-target Host decision; this is not a route
//! or production policy cutover. The Server lifecycle installs the content schema;
//! this fixture installs only its business data and exact-target Host decision.
use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeSnapshot, SnapshotEnvironment, ToolchainContract,
    VirtualPath,
};
use content_history::{HistoryId, ObjectId};
use content_history_adapters::{Snapshot, settings, ticket};
use content_history_store::{Access, Authority, ScopeKey, ScopedStore};
use rusqlite::{Transaction, params};

use crate::{Error, Result, store::SqliteWorkspaceStore};

struct ConfigHost;
impl Authority for ConfigHost {
    fn allows(&self, key: &ScopeKey, _access: Access) -> bool {
        key.workspace == "history-ws" && key.target == "configuration"
    }
}
fn scoped<'a, 'c>(tx: &'a Transaction<'c>) -> ScopedStore<'a, 'c> {
    ScopedStore::new(
        tx,
        ScopeKey {
            workspace: "history-ws".into(),
            target: "configuration".into(),
        },
        &ConfigHost,
    )
}
fn content_error(e: impl std::fmt::Display) -> Error {
    Error::Store(e.to_string())
}
fn sources(value: &str) -> ConfigTreeSnapshot {
    ConfigTreeSnapshot::from_entries([
        ConfigEntry::new(
            VirtualPath::parse("main.dcdl").unwrap(),
            ConfigContentType::Decodal,
            "import \"./parts/value.dcdl\"",
        )
        .unwrap(),
        ConfigEntry::new(
            VirtualPath::parse("parts/value.dcdl").unwrap(),
            ConfigContentType::Decodal,
            format!("# exact comment\r\n{{ title = \"{value}\"; }}\r\n"),
        )
        .unwrap(),
    ])
    .unwrap()
}
fn prepare(s: &ScopedStore<'_, '_>, candidate: &Snapshot) -> Result<()> {
    for (id, object) in &candidate.objects {
        s.put(*id, &object.encode()).map_err(content_error)?;
    }
    Ok(())
}

/// A minimal caller-owned writer, deliberately not installed as the production config API.
/// Its evaluation, exact sources, projection and new history share the Server's transaction.
fn adopt_sources(
    db: &SqliteWorkspaceStore,
    source: &ConfigTreeSnapshot,
    expected: Option<HistoryId>,
    fail_after_adopt: bool,
) -> Result<HistoryId> {
    let candidate = settings::prepare(source).map_err(content_error)?;
    let contract = ToolchainContract::new(1, vec![VirtualPath::parse("main.dcdl").unwrap()], 1);
    let projection = SnapshotEnvironment::new(source.clone())
        .evaluate_contract(&contract)
        .map_err(|e| content_error(format!("{e:?}")))?;
    db.with_immediate_transaction(|tx| {
        let s=scoped(tx); prepare(&s,&candidate)?;
        let head=s.adopt(candidate.root,expected,"authenticated-user".into(),5).map_err(content_error)?;
        tx.execute("INSERT INTO workspace_config_trees(workspace_id,content_digest,schema_version,entrypoints_json,decodal_version,import_policy_version,toolchain_fingerprint,projection_digest,updated_at) VALUES('history-ws',?1,1,'[\"main.dcdl\"]',?2,1,?3,?4,'now') ON CONFLICT(workspace_id) DO UPDATE SET content_digest=excluded.content_digest,projection_digest=excluded.projection_digest",params![source.digest,contract.decodal_version,contract.fingerprint,projection.projection_digest])?;
        tx.execute("DELETE FROM workspace_config_entries WHERE workspace_id='history-ws'",[])?;
        for entry in source.entries.values() {
            tx.execute("INSERT INTO workspace_config_entries VALUES('history-ws',?1,'decodal',?2,?3)",params![entry.path.as_str(),entry.content,entry.content_digest])?;
        }
        if fail_after_adopt { return Err(Error::Store("injected business adoption failure".into())); }
        Ok(head)
    })
}

#[test]
fn server_owned_transaction_adopts_sources_projection_history_and_rolls_back_failed_business_write()
{
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    let db = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    db.with_immediate_transaction(|tx| {
        tx.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('history-owner','user','history-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('history-ws','history-owner','History fixture','active','1','1');")?;
        Ok(())
    }).unwrap();
    let old = sources("旧版");
    let first = adopt_sources(&db, &old, None, false).unwrap();
    let new = sources("新版");
    assert!(adopt_sources(&db, &new, Some(first), true).is_err());
    db.with_immediate_transaction(|tx| {
        let s=scoped(tx);assert_eq!(s.head().map_err(content_error)?,Some(first));
        let record=s.read(ObjectId::History(first)).map_err(content_error)?;
        let content_history::Object::History(history)=record else {panic!("history expected")};
        let restored=settings::restore(history.root,|id|s.read(id).map_err(|e|content_history_adapters::Error::Read(e.to_string()))).map_err(content_error)?;
        assert_eq!(restored,old);
        let digest:String=tx.query_row("SELECT content_digest FROM workspace_config_trees WHERE workspace_id='history-ws'",[],|r|r.get(0))?;
        assert_eq!(digest,old.digest);
        let text:String=tx.query_row("SELECT content FROM workspace_config_entries WHERE workspace_id='history-ws' AND path='parts/value.dcdl'",[],|r|r.get(0))?;
        assert_eq!(text,old.entries[&VirtualPath::parse("parts/value.dcdl").unwrap()].content);
        Ok(())
    }).unwrap();
    let second = adopt_sources(&db, &new, Some(first), false).unwrap();
    assert_ne!(first, second);
    assert!(adopt_sources(&db, &old, Some(first), false).is_err());
    drop(db);
    let db = SqliteWorkspaceStore::open_for_test(path).unwrap();
    db.with_immediate_transaction(|tx| {
        let s=scoped(tx);assert_eq!(s.head().map_err(content_error)?,Some(second));
        let projection:String=tx.query_row("SELECT projection_digest FROM workspace_config_trees WHERE workspace_id='history-ws'",[],|r|r.get(0))?;
        let contract=ToolchainContract::new(1,vec![VirtualPath::parse("main.dcdl").unwrap()],1);
        assert_eq!(projection,SnapshotEnvironment::new(new.clone()).evaluate_contract(&contract).unwrap().projection_digest);
        assert!(s.snapshot(first).map_err(content_error)?.values().any(|o|matches!(o,content_history::Object::Blob(b) if b==old.entries[&VirtualPath::parse("parts/value.dcdl").unwrap()].content.as_bytes())));
        Ok(())
    }).unwrap();
}

#[test]
fn ticket_snapshot_and_history_restoration_do_not_rewind_separate_business_events() {
    let db = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    db.with_immediate_transaction(|tx| {
        // Business state is deliberately not a field accepted by the content adapter.
        tx.execute_batch("CREATE TEMP TABLE business_events(kind TEXT,value TEXT); INSERT INTO business_events VALUES('state','closed'),('assignee','current-worker'),('relation','depends-on-current');")?;
        let s=scoped(tx);
        let doc=ticket::Document {ticket_id:"fixture-ticket".into(),title:"Title".into(),initial_body:"old body\r\n".into(),initial_author:"original-user".into(),posts:vec![ticket::Post{id:"post-z".into(),author:"original-poster".into(),body:"post body".into()}]};
        let original=ticket::prepare(&doc).map_err(content_error)?;prepare(&s,&original)?;
        let first=s.adopt(original.root,None,"first-editor".into(),1).map_err(content_error)?;
        let mut changed=doc.clone();changed.initial_body="edited body".into();
        let candidate=ticket::prepare(&changed).map_err(content_error)?;prepare(&s,&candidate)?;
        let second=s.adopt(candidate.root,Some(first),"second-editor".into(),2).map_err(content_error)?;
        let third=s.adopt(original.root,Some(second),"restore-editor".into(),3).map_err(content_error)?;
        assert_ne!(first,third);
        let restored=ticket::restore(original.root,"fixture-ticket",|id|s.read(id).map_err(|e|content_history_adapters::Error::Read(e.to_string()))).map_err(content_error)?;
        assert_eq!(restored,doc);
        let mut q=tx.prepare("SELECT kind,value FROM business_events ORDER BY kind")?;
        let events=q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
        assert_eq!(events,vec![("assignee".into(),"current-worker".into()),("relation".into(),"depends-on-current".into()),("state".into(),"closed".into())]);
        Ok(())
    }).unwrap();
}
