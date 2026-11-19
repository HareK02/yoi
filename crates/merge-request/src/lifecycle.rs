use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MergeRequestState {
    Implementation {},
    Integration {
        started_by: String,
    },
    Completed {
        completion: MergeRequestCompletion,
    },
    Canceled {
        from: MergeRequestCancellationOrigin,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MergeRequestCompletion {
    Published {
        started_by: String,
        merge_event_id: String,
    },
    /// Historical integration entry was not observed; missing results remain diagnostic.
    Legacy { merge_event_id: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MergeRequestCancellationOrigin {
    Implementation {},
    Integration { started_by: String },
    Legacy {},
}

impl MergeRequestState {
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Implementation {} | Self::Integration { .. })
    }

    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }

    pub fn is_canceled(&self) -> bool {
        matches!(self, Self::Canceled { .. })
    }

    pub fn started_by(&self) -> Option<&str> {
        match self {
            Self::Integration { started_by }
            | Self::Completed {
                completion: MergeRequestCompletion::Published { started_by, .. },
            }
            | Self::Canceled {
                from: MergeRequestCancellationOrigin::Integration { started_by },
            } => Some(started_by),
            _ => None,
        }
    }
}

/// List selectors preserve the historical grouping, not the lifecycle representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestStateFilter {
    Open,
    Merged,
    Closed,
}

impl MergeRequestStateFilter {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

// JSON checks use COALESCE because SQLite CHECK otherwise accepts unknown (NULL).
// Removing every permitted member rejects extra and duplicate fields at each level.
pub(super) fn add_lifecycle_column(c: &Connection, name: &str) -> Result<(), MergeRequestError> {
    c.execute_batch(&r#"
        ALTER TABLE merge_requests ADD COLUMN lifecycle TEXT NOT NULL DEFAULT '{"kind":"implementation"}'
        CHECK (CASE WHEN json_valid(lifecycle) THEN COALESCE(
            json_type(lifecycle)='object' AND
            CASE json_extract(lifecycle,'$.kind')
                WHEN 'implementation' THEN json_remove(lifecycle,'$.kind')='{}'
                WHEN 'integration' THEN
                    json_type(lifecycle,'$.started_by')='text' AND
                    json_remove(lifecycle,'$.kind','$.started_by')='{}'
                WHEN 'completed' THEN
                    json_remove(lifecycle,'$.kind','$.completion')='{}' AND
                    json_type(lifecycle,'$.completion')='object' AND
                    CASE json_extract(lifecycle,'$.completion.kind')
                        WHEN 'published' THEN
                            json_type(lifecycle,'$.completion.started_by')='text' AND
                            json_type(lifecycle,'$.completion.merge_event_id')='text' AND
                            json_remove(json_extract(lifecycle,'$.completion'),'$.kind','$.started_by','$.merge_event_id')='{}'
                        WHEN 'legacy' THEN
                            (json_type(lifecycle,'$.completion.merge_event_id') IS NULL OR
                             json_type(lifecycle,'$.completion.merge_event_id') IN ('text','null')) AND
                            json_remove(json_extract(lifecycle,'$.completion'),'$.kind','$.merge_event_id')='{}'
                        ELSE 0 END
                WHEN 'canceled' THEN
                    json_remove(lifecycle,'$.kind','$.from')='{}' AND
                    json_type(lifecycle,'$.from')='object' AND
                    CASE json_extract(lifecycle,'$.from.kind')
                        WHEN 'implementation' THEN json_remove(json_extract(lifecycle,'$.from'),'$.kind')='{}'
                        WHEN 'integration' THEN
                            json_type(lifecycle,'$.from.started_by')='text' AND
                            json_remove(json_extract(lifecycle,'$.from'),'$.kind','$.started_by')='{}'
                        WHEN 'legacy' THEN json_remove(json_extract(lifecycle,'$.from'),'$.kind')='{}'
                        ELSE 0 END
                ELSE 0 END, 0) ELSE 0 END);
    "#.replace("lifecycle", name))?;
    Ok(())
}

pub(super) fn migrate_lifecycle_column(c: &Connection) -> Result<(), MergeRequestError> {
    add_lifecycle_column(c, "lifecycle")?;
    c.execute_batch(r#"
        UPDATE merge_requests SET lifecycle=CASE state
            WHEN 'open' THEN json_object('kind','implementation')
            WHEN 'merged' THEN json_object('kind','completed','completion',json_object(
                'kind','legacy','merge_event_id',(
                    SELECT event_id FROM merge_request_thread_events e
                    WHERE e.workspace_id=merge_requests.workspace_id AND e.merge_request_id=merge_requests.merge_request_id
                    AND e.kind='merge' ORDER BY e.sequence DESC LIMIT 1)))
            WHEN 'closed' THEN json_object('kind','canceled','from',json_object('kind','legacy'))
            ELSE NULL END;
        ALTER TABLE merge_requests DROP COLUMN state;
        ALTER TABLE merge_requests RENAME COLUMN lifecycle TO state;
    "#)?;
    Ok(())
}

pub(super) fn lifecycle_triggers(c: &Connection) -> Result<(), MergeRequestError> {
    c.execute_batch(r#"
        CREATE TRIGGER merge_request_lifecycle_transition BEFORE UPDATE OF state ON merge_requests
        WHEN NEW.state IS NOT OLD.state AND NOT COALESCE(
            (json_extract(OLD.state,'$.kind')='implementation' AND (
                json_extract(NEW.state,'$.kind')='integration' OR
                (json_extract(NEW.state,'$.kind')='canceled' AND json_extract(NEW.state,'$.from.kind')='implementation'))) OR
            (json_extract(OLD.state,'$.kind')='integration' AND (
                (json_extract(NEW.state,'$.kind')='completed' AND json_extract(NEW.state,'$.completion.kind')='published' AND
                 json_extract(NEW.state,'$.completion.started_by')=json_extract(OLD.state,'$.started_by')) OR
                (json_extract(NEW.state,'$.kind')='canceled' AND json_extract(NEW.state,'$.from.kind')='integration' AND
                 json_extract(NEW.state,'$.from.started_by')=json_extract(OLD.state,'$.started_by')))),0)
        BEGIN SELECT RAISE(ABORT,'invalid lifecycle transition'); END;
    "#)?;
    for operation in ["INSERT", "UPDATE OF state"] {
        let suffix = if operation == "INSERT" {
            "insert"
        } else {
            "update"
        };
        c.execute_batch(&format!(r#"
            CREATE TRIGGER merge_request_lifecycle_evidence_{suffix} BEFORE {operation} ON merge_requests
            WHEN (
                COALESCE(json_extract(NEW.state,'$.started_by'),json_extract(NEW.state,'$.completion.started_by'),json_extract(NEW.state,'$.from.started_by')) IS NOT NULL
                AND NOT EXISTS(SELECT 1 FROM merge_request_thread_events e
                    WHERE e.workspace_id=NEW.workspace_id AND e.merge_request_id=NEW.merge_request_id
                    AND e.event_id=COALESCE(json_extract(NEW.state,'$.started_by'),json_extract(NEW.state,'$.completion.started_by'),json_extract(NEW.state,'$.from.started_by'))
                    AND e.kind='review' AND json_extract(e.payload_json,'$.decision')='approve'))
                OR (json_extract(NEW.state,'$.completion.kind')='published' AND NOT EXISTS(
                    SELECT 1 FROM merge_request_thread_events m JOIN merge_request_thread_events r
                    ON r.workspace_id=m.workspace_id AND r.merge_request_id=m.merge_request_id
                    WHERE m.workspace_id=NEW.workspace_id AND m.merge_request_id=NEW.merge_request_id
                    AND m.event_id=json_extract(NEW.state,'$.completion.merge_event_id') AND m.kind='merge'
                    AND r.event_id=json_extract(NEW.state,'$.completion.started_by') AND r.sequence<m.sequence))
            BEGIN SELECT RAISE(ABORT,'lifecycle requires retained evidence'); END;
        "#))?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeRequestPhase {
    Implementation,
    Integration,
    Completed,
    Canceled,
}

pub(super) fn verify_lifecycle_schema(c: &Connection) -> Result<(), MergeRequestError> {
    c.prepare("SELECT state FROM merge_requests LIMIT 0")?;
    let mut states = c.prepare("SELECT workspace_id,merge_request_id,state FROM merge_requests")?;
    let rows = states.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (ws, mr, state) = row?;
        let state: MergeRequestState = json(&state)?;
        if let Some(started_by) = state.started_by() {
            let valid: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_thread_events WHERE workspace_id=?1 AND merge_request_id=?2 AND event_id=?3 AND kind='review' AND json_extract(payload_json,'$.decision')='approve')", params![ws,mr,started_by], |r| r.get(0))?;
            if !valid {
                return Err(MergeRequestError::Corrupt(
                    "invalid integration entry".into(),
                ));
            }
        }
        if let MergeRequestState::Completed {
            completion:
                MergeRequestCompletion::Published {
                    started_by,
                    merge_event_id,
                },
        } = state
        {
            let valid: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM merge_request_thread_events m JOIN merge_request_thread_events r ON r.workspace_id=m.workspace_id AND r.merge_request_id=m.merge_request_id WHERE m.workspace_id=?1 AND m.merge_request_id=?2 AND m.event_id=?3 AND m.kind='merge' AND r.event_id=?4 AND r.sequence<m.sequence)", params![ws,mr,merge_event_id,started_by], |r| r.get(0))?;
            if !valid {
                return Err(MergeRequestError::Corrupt(
                    "invalid published completion".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Old review submission did not fence requirement changes and saved no entry
/// evidence. Even a matching current digest cannot prove validity at submission
/// (requirements may have changed and returned). Keep old open MRs in implementation
/// until a fresh valid review; preserve every review and published result unchanged.
/// Terminal rows retain explicit legacy provenance, including absent merge results.
pub(super) fn migrate_v13_lifecycle(c: &Connection) -> Result<(), MergeRequestError> {
    let transaction = c.unchecked_transaction()?;
    verify(&transaction)?;
    lifecycle::migrate_lifecycle_column(&transaction)?;
    lifecycle_triggers(&transaction)?;
    transaction.execute(
        "UPDATE merge_request_schema SET version=14 WHERE singleton=1",
        [],
    )?;
    verify_lifecycle_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}
