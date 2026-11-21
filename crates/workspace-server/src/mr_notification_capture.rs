//! MR facts share the existing notification ledger. Capture snapshots responsibility;
//! recovery never reassigns an old fact to today's owner or interprets comment prose.
use super::*;

pub(in crate::store) const MIGRATION_NAME: &str =
    "MR comments publication and owner notification capture";

fn capture(input: &str, kind: &str, actor: &str) -> String {
    let subject = subject_facts_sql();
    let facts = format!(
        "CASE roles.role WHEN 'orchestrator' THEN {subject} ELSE json_object('principal_kind','worker','runtime_id',json_extract(owner.payload_json,'$.worker.runtime_id'),'worker_id',json_extract(owner.payload_json,'$.worker.worker_id'),'case_state',t.workflow_state,'released',json('false')) END"
    );
    format!("INSERT OR IGNORE INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,actor,event_index,facts_json)
 SELECT NEW.workspace_id,r.ticket_id,{input},roles.role,
 CASE roles.role WHEN 'orchestrator' THEN COALESCE(a.assignment_id,'') ELSE CASE WHEN json_extract(owner.payload_json,'$.worker.worker_id') IS NOT NULL THEN owner.operation_id ELSE '' END END,
 {kind},{actor},NULL,json_set({facts},'$.merge_request_id',NEW.merge_request_id)
 FROM merge_request_ticket_relations r JOIN typed_tickets t ON t.workspace_id=r.workspace_id AND t.ticket_id=r.ticket_id
 CROSS JOIN (SELECT 'orchestrator' role UNION ALL SELECT 'mr_worker') roles
 LEFT JOIN ticket_current_worker_assignments c ON c.workspace_id=t.workspace_id AND c.ticket_id=t.ticket_id AND c.role='orchestrator'
 LEFT JOIN ticket_worker_assignments a ON a.workspace_id=c.workspace_id AND a.assignment_id=c.assignment_id
 LEFT JOIN merge_request_thread_events owner ON owner.workspace_id=r.workspace_id AND owner.merge_request_id=r.merge_request_id AND owner.sequence=(SELECT MAX(e.sequence) FROM merge_request_thread_events e WHERE e.workspace_id=r.workspace_id AND e.merge_request_id=r.merge_request_id AND e.kind='worker_assigned')
 WHERE r.workspace_id=NEW.workspace_id AND r.merge_request_id=NEW.merge_request_id;")
}

pub(in crate::store) fn install(conn: &Connection) -> Result<()> {
    let actor_path = "CASE NEW.kind WHEN 'review_requested' THEN '$.requested_by' WHEN 'review' THEN '$.reviewer' WHEN 'review_revoked' THEN '$.revoked_by' WHEN 'merge' THEN '$.merged_by' WHEN 'comment' THEN '$.author' WHEN 'worker_assigned' THEN '$.assigned_by' END";
    let actor = format!(
        "'worker:' || json_extract(NEW.payload_json,({actor_path}) || '.runtime_id') || '/' || json_extract(NEW.payload_json,({actor_path}) || '.worker_id')"
    );
    let event = capture(
        "'mr-event:' || NEW.merge_request_id || ':' || NEW.event_id",
        "'mr_' || NEW.kind",
        &actor,
    );
    let source = capture(
        "'mr-source:' || NEW.merge_request_id || ':' || NEW.sequence",
        "'mr_source_changed'",
        "NULL",
    );
    // Replace only future capture. Prior requests, ACKs and recipient snapshots
    // remain immutable; historical comments have no event-time Subject snapshot.
    conn.execute_batch(&format!("DROP TRIGGER notification_mr_event;
 DROP TRIGGER notification_mr_source;
 CREATE TRIGGER notification_mr_event AFTER INSERT ON merge_request_thread_events BEGIN {event} END;
 CREATE TRIGGER notification_mr_source AFTER INSERT ON merge_request_source_observations WHEN NEW.previous_subject_ref IS NOT NULL BEGIN {source} END;"))?;
    Ok(())
}

pub(in crate::store) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 137 {
        return Err(Error::Store(
            "MR notification migration requires schema 137".into(),
        ));
    }
    merge_request::migrate(conn)?;
    let tx = conn.unchecked_transaction()?;
    install(&tx)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(138,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}
