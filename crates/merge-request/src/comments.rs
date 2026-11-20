//! Integration explanations are ordinary thread comments, not an aggregate or
//! scheduling schema. Only replay and owner fencing are machine-managed.
use super::*;

impl MergeRequestStore {
    pub fn comment(
        &self,
        auth: &MergeRequestAuth,
        mr_id: &str,
        operation_id: &str,
        body: &str,
        now: DateTime<Utc>,
        admit: impl Fn(&Connection) -> Result<(), MergeRequestError>,
    ) -> Result<CommentEvent, MergeRequestError> {
        nonempty("operation_id", operation_id)?;
        nonempty("body", body)?;
        bounded_body("comment", body)?;
        let mut c = self.lock()?;
        let t = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        admit(&t)?;
        let mr = load_mr(&t, &auth.workspace_id, mr_id)?.ok_or(MergeRequestError::NotFound)?;
        self.current_worker_auth(auth, &mr)?;
        if let Some(event) = mr.thread.iter().find_map(|event| match event {
            MergeRequestThreadEvent::Comment(event)
                if event.operation_id.as_deref() == Some(operation_id) =>
            {
                Some(event)
            }
            _ => None,
        }) {
            if event.body != body
                || event.owner_operation_id.as_deref() != Some(&auth.assignment_id)
                || event.author.runtime_id != auth.runtime_id
                || event.author.worker_id != auth.worker_id
            {
                return Err(MergeRequestError::Conflict(
                    "comment operation fingerprint mismatch".into(),
                ));
            }
            return Ok(event.clone());
        }
        let event = CommentEvent {
            event_id: Uuid::now_v7().to_string(),
            sequence: next_seq(&t, &auth.workspace_id, mr_id)?,
            operation_id: Some(operation_id.into()),
            owner_operation_id: Some(auth.assignment_id.clone()),
            body: body.into(),
            author: WorkerIdentity {
                runtime_id: auth.runtime_id.clone(),
                worker_id: auth.worker_id.clone(),
            },
            created_at: now,
        };
        insert_event(
            &t,
            &auth.workspace_id,
            mr_id,
            "comment",
            &event,
            now,
            Some(operation_id),
        )?;
        t.execute(
            "UPDATE merge_requests SET updated_at=?3 WHERE workspace_id=?1 AND merge_request_id=?2",
            params![auth.workspace_id, mr_id, now.to_rfc3339()],
        )?;
        t.commit()?;
        Ok(event)
    }
}
