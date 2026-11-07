//! Frozen schema-96 Job notification import. Only schema construction/migration
//! may call this module; current routing never reads the old delivery ledger.
use super::*;
// Only these frozen notification fields are needed; executable grants and
// request envelopes changed after this boundary and are not decoded here.
#[derive(Deserialize)]
struct LegacyNotificationRequest {
    purpose: String,
    input: serde_json::Value,
    #[serde(default)]
    notification_target: Option<RuntimeWorkerRef>,
}

// Frozen serialized evidence shape, not a current delivery API or state machine.
#[derive(Serialize)]
struct LegacyDeliveryRecord {
    workspace_id: String,
    delivery_id: String,
    job_id: String,
    attempt_id: String,
    target: RuntimeWorkerRef,
    state: LegacyDeliveryState,
    failure_category: Option<String>,
    failure_detail: Option<String>,
    created_at: String,
    updated_at: String,
    delivered_at: Option<String>,
}

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LegacyDeliveryState {
    Pending,
    Sending,
    Completed,
    Failed,
    Unknown,
}

impl LegacyDeliveryState {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "sending" => Ok(Self::Sending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "unknown" => Ok(Self::Unknown),
            other => Err(Error::Store(format!(
                "unknown Backend Job delivery state `{other}`"
            ))),
        }
    }
}

pub(in crate::store) fn install(conn: &Connection) -> Result<()> {
    // Generalize the existing source parent without changing any sequence, request,
    // or receipt. The migration caller disables FK enforcement for this rebuild.
    let triggers = conn
        .prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger'")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let indexes = conn.prepare("SELECT sql FROM sqlite_schema WHERE type='index' AND tbl_name='notification_sources' AND sql IS NOT NULL")?
        .query_map([], |r| r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, _) in &triggers {
        conn.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
    }
    conn.execute_batch(
        "CREATE TABLE notification_sources_job_upgrade (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id TEXT NOT NULL, ticket_id TEXT, input_event_id TEXT NOT NULL,
        role TEXT NOT NULL, assignment_id TEXT NOT NULL, kind TEXT NOT NULL,
        actor TEXT, event_index INTEGER, facts_json TEXT NOT NULL CHECK(json_valid(facts_json)),
        CHECK(ticket_id IS NOT NULL OR role='backend_job'),
        UNIQUE(workspace_id,ticket_id,input_event_id,role,assignment_id),
        FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id));
        INSERT INTO notification_sources_job_upgrade SELECT * FROM notification_sources;
        DROP TABLE notification_sources;
        ALTER TABLE notification_sources_job_upgrade RENAME TO notification_sources;",
    )?;
    for sql in indexes {
        conn.execute_batch(&sql)?;
    }
    for (_, sql) in triggers {
        conn.execute_batch(&sql)?;
    }
    conn.execute_batch("CREATE TRIGGER notification_job_source_guard BEFORE INSERT ON notification_sources WHEN NEW.role='backend_job' BEGIN
        SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM backend_jobs j JOIN backend_job_attempts a USING(workspace_id,job_id)
        WHERE j.workspace_id=NEW.workspace_id AND j.job_id=json_extract(NEW.facts_json,'$.backend_job.job_id') AND a.attempt_id=json_extract(NEW.facts_json,'$.backend_job.attempt_id')
        AND j.state='completed' AND a.state='completed' AND a.attempt=j.current_attempt AND j.result_digest=json_extract(NEW.facts_json,'$.result_digest'))
        THEN RAISE(ABORT,'Job notification requires a committed current result') END;
        END;
        CREATE UNIQUE INDEX notification_job_identity ON notification_sources(workspace_id,input_event_id) WHERE role='backend_job';
        CREATE TABLE notification_job_targets (
            workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, target_json TEXT NOT NULL CHECK(json_valid(target_json)),
            PRIMARY KEY(workspace_id,delivery_id),
            FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_requests(workspace_id,delivery_id));
        CREATE TRIGGER notification_job_targets_no_update BEFORE UPDATE ON notification_job_targets BEGIN SELECT RAISE(ABORT,'Job notification target is immutable'); END;
        CREATE TRIGGER notification_job_targets_no_delete BEFORE DELETE ON notification_job_targets BEGIN SELECT RAISE(ABORT,'Job notification target is immutable'); END;
        CREATE TABLE notification_job_legacy_receipts (
            workspace_id TEXT NOT NULL, delivery_id TEXT NOT NULL, record_json TEXT NOT NULL CHECK(json_valid(record_json)),
            runtime_accepted INTEGER NOT NULL CHECK(runtime_accepted IN (0,1)),
            PRIMARY KEY(workspace_id,delivery_id),
            FOREIGN KEY(workspace_id,delivery_id) REFERENCES notification_decisions(workspace_id,delivery_id));
        CREATE TRIGGER notification_job_legacy_no_update BEFORE UPDATE ON notification_job_legacy_receipts BEGIN SELECT RAISE(ABORT,'Legacy notification evidence is immutable'); END;
        CREATE TRIGGER notification_job_legacy_no_delete BEFORE DELETE ON notification_job_legacy_receipts BEGIN SELECT RAISE(ABORT,'Legacy notification evidence is immutable'); END;")?;
    // Preparation may fail before a routing decision exists. Keep observations
    // against the common source, never invent a terminal decision to hold them.
    conn.execute_batch("CREATE TABLE notification_source_preparation_failures (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT,
        source_sequence INTEGER NOT NULL REFERENCES notification_sources(sequence),
        code TEXT NOT NULL,
        recorded_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')));
        CREATE INDEX notification_source_preparation_failures_source ON notification_source_preparation_failures(source_sequence,sequence);
        CREATE TRIGGER notification_source_preparation_failures_no_update BEFORE UPDATE ON notification_source_preparation_failures BEGIN SELECT RAISE(ABORT,'Source preparation evidence is immutable'); END;
        CREATE TRIGGER notification_source_preparation_failures_no_delete BEFORE DELETE ON notification_source_preparation_failures BEGIN SELECT RAISE(ABORT,'Source preparation evidence is immutable'); END;")?;
    // Mutable scheduling only, independent of immutable source/decision evidence.
    conn.execute_batch(
        "CREATE TABLE notification_source_offers (
        source_sequence INTEGER PRIMARY KEY REFERENCES notification_sources(sequence),
        last_offered INTEGER NOT NULL CHECK(last_offered > 0));",
    )?;
    // Includes no-target results and the crash gap before the former reservation.
    let identities = conn.prepare("SELECT a.workspace_id,a.job_id,a.attempt_id FROM backend_job_attempts a JOIN backend_jobs j USING(workspace_id,job_id) WHERE a.state='completed' AND j.state='completed' AND a.attempt=j.current_attempt")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (workspace, job, attempt) in identities {
        let id = retained_delivery_id(conn, &workspace, &job, &attempt)?;
        let snapshot = read_result_snapshot(conn, &workspace, &job)?;
        capture_result(
            conn,
            &workspace,
            &job,
            &attempt,
            &id,
            &snapshot.request.purpose,
            &snapshot.request.notification_target,
            snapshot.result_digest.as_deref(),
        )?;
        import_legacy(conn, &workspace, &job, &attempt, &id)?;
    }
    Ok(())
}

/// Historical schema validation only. Schema 130 removes these objects; current
/// validation must use the parent's `verify` without requiring legacy tables.
pub(in crate::store) fn verify_schema96(conn: &Connection) -> Result<()> {
    super::verify(conn)?;
    for name in [
        "notification_job_legacy_receipts",
        "notification_job_legacy_no_update",
        "notification_job_legacy_no_delete",
    ] {
        let present: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |r| r.get(0),
        )?;
        if !present {
            return Err(Error::Store(format!(
                "Job notification constraint {name} is missing"
            )));
        }
    }
    Ok(())
}

struct NotificationResultSnapshot {
    request: LegacyNotificationRequest,
    result: Option<serde_json::Value>,
    result_digest: Option<String>,
}

// Schema 96 precedes later Job lifecycle migrations. Never read the executable
// Job's current-schema columns while importing this historical result envelope.
fn read_result_snapshot(
    conn: &Connection,
    workspace: &str,
    job: &str,
) -> Result<NotificationResultSnapshot> {
    let (request, result, result_digest): (String, Option<String>, Option<String>) = conn.query_row(
        "SELECT request_json, result_json, result_digest FROM backend_jobs WHERE workspace_id=?1 AND job_id=?2",
        params![workspace, job],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(NotificationResultSnapshot {
        request: decode(&request)?,
        result: result.map(|value| decode(&value)).transpose()?,
        result_digest,
    })
}

// Old rows did not store request bytes. Only the original deterministic renderers
// may reconstruct an interrupted request; no current Ticket contents participate.
// Accepted legacy rows keep their actual evidence, never a fabricated new request.
fn import_legacy(
    conn: &Connection,
    workspace: &str,
    job_id: &str,
    attempt_id: &str,
    id: &str,
) -> Result<()> {
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM notification_decisions WHERE workspace_id=?1 AND delivery_id=?2)",params![workspace,id],|r|r.get(0))?;
    if exists {
        return Ok(());
    }
    let Some(old) = read_delivery(conn, workspace, id)? else {
        return Ok(());
    };
    let accepted = old.state == LegacyDeliveryState::Completed && old.failure_category.is_none();
    let count:i64=conn.query_row("SELECT COUNT(*) FROM backend_job_deliveries WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3",params![workspace,job_id,attempt_id],|r|r.get(0))?;
    let mut content = None;
    let mut failure = legacy_failure(&old);
    let (disposition, reason) = match old.failure_category.as_deref() {
        _ if accepted => ("deliver", NotificationReason::Result),
        _ if count > 1 => {
            failure = Some(NotificationFailureCode::PreparationFailed);
            ("defer", NotificationReason::SourceUnavailable)
        }
        Some("notification_suppressed_no_findings") => ("suppress", NotificationReason::NoFindings),
        Some("notification_suppressed_stale_ticket_content") => {
            ("suppress", NotificationReason::StaleContent)
        }
        _ if old.state == LegacyDeliveryState::Completed => {
            ("defer", NotificationReason::SourceUnavailable)
        }
        Some(
            "invalid_checker_input"
            | "invalid_checker_result"
            | "current_ticket_content_unavailable"
            | "notification_too_large",
        ) => ("defer", NotificationReason::SourceUnavailable),
        _ => {
            let snapshot = read_result_snapshot(conn, workspace, job_id)?;
            let facts = if snapshot.request.notification_target.as_ref() != Some(&old.target) {
                JobNotificationFacts::Failed(NotificationFailureCode::PreparationFailed)
            } else {
                legacy_result_facts(&snapshot, job_id, attempt_id)
            };
            let decision = policy(snapshot.request.notification_target.is_some(), &facts);
            match facts {
                JobNotificationFacts::Result(body) => content = Some(body),
                JobNotificationFacts::Failed(code) => failure = Some(code),
                // A valid empty result could not have passed the old sender's
                // findings gate even if it crashed before recording suppression.
                JobNotificationFacts::NoFindings => failure = None,
                JobNotificationFacts::StaleContent => {}
            }
            decision
        }
    };
    save_decision(conn, workspace, id, disposition, reason)?;
    conn.execute(
        "INSERT INTO notification_job_legacy_receipts VALUES(?1,?2,?3,?4)",
        params![workspace, id, json(&old)?, accepted],
    )?;
    if !accepted && disposition != "suppress" {
        if let Some(failure) = failure {
            save_failure(conn, workspace, id, failure)?;
        }
    }
    if disposition == "deliver" {
        if let Some(content) = content {
            conn.execute(
                "INSERT INTO notification_requests VALUES(?1,?2,?3,?4)",
                params![workspace, id, content, json(&old.target)?],
            )?;
        }
    }
    Ok(())
}

fn legacy_result_facts(
    snapshot: &NotificationResultSnapshot,
    job_id: &str,
    attempt_id: &str,
) -> JobNotificationFacts {
    let content = if snapshot.request.purpose == crate::ticket_item_checker::PURPOSE {
        let Some(result) = &snapshot.result else {
            return JobNotificationFacts::Failed(NotificationFailureCode::InvalidResult);
        };
        match crate::ticket_item_checker::validate_result(&snapshot.request.input, result) {
            Ok(result) if result.findings.is_empty() => return JobNotificationFacts::NoFindings,
            Ok(_) => {}
            Err(_) => return JobNotificationFacts::Failed(NotificationFailureCode::InvalidResult),
        }
        crate::ticket_item_checker::render_result(&snapshot.request.input, result)
    } else {
        Ok(format!(
            "Backend Job `{job_id}` completed attempt `{attempt_id}` for `{}`. Durable result digest: {}.",
            snapshot.request.purpose,
            snapshot.result_digest.as_deref().unwrap_or("unavailable")
        ))
    };
    match content {
        Ok(content) if content.len() > crate::backend_job::MAX_JOB_DELIVERY_BYTES => {
            JobNotificationFacts::Failed(NotificationFailureCode::ContentTooLarge)
        }
        Ok(content) if !content.is_empty() && !content.contains('\0') => {
            JobNotificationFacts::Result(content)
        }
        _ => JobNotificationFacts::Failed(NotificationFailureCode::RenderingFailed),
    }
}

fn legacy_failure(old: &LegacyDeliveryRecord) -> Option<NotificationFailureCode> {
    use NotificationFailureCode as F;
    match old.failure_category.as_deref() {
        Some("notification_outcome_unknown") => Some(F::AcceptanceUnknown),
        Some("notification_rejected") => Some(F::RuntimeRejected),
        Some("notification_too_large") => Some(F::ContentTooLarge),
        Some("invalid_checker_input" | "invalid_checker_result") => Some(F::InvalidResult),
        Some("current_ticket_content_unavailable") => Some(F::SourceLookupFailed),
        _ if matches!(
            old.state,
            LegacyDeliveryState::Sending
                | LegacyDeliveryState::Unknown
                | LegacyDeliveryState::Completed
        ) =>
        {
            Some(F::AcceptanceUnknown)
        }
        _ => None,
    }
}

fn retained_delivery_id(
    conn: &Connection,
    workspace: &str,
    job: &str,
    attempt: &str,
) -> Result<String> {
    // Retained receipts are authoritative even for a non-derived identity. An ACK
    // wins multiple identities; otherwise the importer defers the ambiguity.
    let saved:Option<String>=conn.query_row("SELECT delivery_id FROM backend_job_deliveries WHERE workspace_id=?1 AND job_id=?2 AND attempt_id=?3 ORDER BY (state='completed' AND failure_category IS NULL) DESC,created_at,delivery_id LIMIT 1",params![workspace,job,attempt],|r|r.get(0)).optional()?;
    Ok(saved.unwrap_or_else(|| crate::backend_job::delivery_id(job, attempt)))
}

fn read_delivery(
    conn: &Connection,
    workspace_id: &str,
    delivery_id: &str,
) -> Result<Option<LegacyDeliveryRecord>> {
    let row = conn.query_row(
        "SELECT job_id, attempt_id, target_runtime_id, target_worker_id, state, failure_category, failure_detail, created_at, updated_at, delivered_at FROM backend_job_deliveries WHERE workspace_id = ?1 AND delivery_id = ?2",
        params![workspace_id, delivery_id],
        |row| Ok((
            row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?,
            row.get::<_, String>(4)?, row.get::<_, Option<String>>(5)?, row.get::<_, Option<String>>(6)?,
            row.get::<_, String>(7)?, row.get::<_, String>(8)?, row.get::<_, Option<String>>(9)?,
        )),
    ).optional()?;
    let Some((
        job_id,
        attempt_id,
        runtime_id,
        worker_id,
        state,
        failure_category,
        failure_detail,
        created_at,
        updated_at,
        delivered_at,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(LegacyDeliveryRecord {
        workspace_id: workspace_id.to_string(),
        delivery_id: delivery_id.to_string(),
        job_id,
        attempt_id,
        target: RuntimeWorkerRef::new(runtime_id, worker_id),
        state: LegacyDeliveryState::parse(&state)?,
        failure_category,
        failure_detail,
        created_at,
        updated_at,
        delivered_at,
    }))
}
