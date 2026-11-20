//! Core grants follow the issuing Subject, not its replaceable body. Immutable
//! policy mutation receipts prove uninterrupted authority from the issuance CAS
//! identity; the current policy alone cannot distinguish regrant from continuity.
use super::*;
use rusqlite::OptionalExtension;
use server_api::authorization::CurrentSubjectPolicy as CanonicalPolicy;
use server_api::subject_policy::{
    SubjectPermissionPolicyMutation, SubjectPermissionPolicyMutationResponse,
};
use std::collections::HashMap;

impl WorkerAuthorityView {
    pub(super) fn continuous_subject_policies(
        &self,
        subject_id: &str,
        source_change_id: &str,
    ) -> ApiResult<Vec<CanonicalPolicy>> {
        // Keep Subject existence/retirement checks, without demanding that the
        // historical issuer still owns a body (or even remains a live Worker).
        let Some(CurrentSubjectPolicy::Canonical(current)) = self.policy(subject_id)? else {
            return Err(denied("Subject delegation authority is unset"));
        };
        if current.change_id == source_change_id {
            return Ok(vec![current]);
        }
        let records = self.config_store.with_conn(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let raw: Option<String> = transaction.query_row(
                "SELECT policy_json FROM workspace_configuration_policies WHERE workspace_id=?1 AND subject_id=?2",
                rusqlite::params![self.workspace_id, subject_id],
                |row| row.get(0),
            ).optional()?;
            let observed = raw.map(|raw| {
                serde_json::from_str::<crate::policy_config::AdoptedSubjectPolicy>(&raw)
                    .map(|policy| policy.current())
                    .map_err(|error| Error::RegistryInconsistency(error.to_string()))
            }).transpose()?;
            if observed.as_ref() != Some(&current) {
                return Err(Error::WorkspacePermissionDenied(
                    "Subject policy changed during continuity observation".into(),
                ));
            }
            let mut statement = transaction.prepare(
                "SELECT request_json,result_json FROM subject_permission_operations WHERE workspace_id=?1 AND subject_id=?2",
            )?;
            let rows = statement.query_map(rusqlite::params![self.workspace_id, subject_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })?;
        let mut changes = HashMap::new();
        for (request, response) in records {
            let request: SubjectPermissionPolicyMutation = serde_json::from_str(&request)
                .map_err(|_| denied("invalid Subject policy continuity request"))?;
            let response: SubjectPermissionPolicyMutationResponse = serde_json::from_str(&response)
                .map_err(|_| denied("invalid Subject policy continuity response"))?;
            let change_id = response
                .change_id
                .ok_or_else(|| denied("missing Subject policy change identity"))?;
            // Receipts retain request reference order; the current-policy store
            // groups Set references before Namespace references. Compare the
            // same storage projection, not those incidental wire orderings.
            let policy = response.policy.map(|policy| {
                crate::policy_config::AdoptedSubjectPolicy::from_current(subject_id, &policy)
                    .current()
            });
            if changes
                .insert(change_id, (request.expected_change_id, policy))
                .is_some()
            {
                return Err(denied("ambiguous Subject policy continuity history"));
            }
        }
        let mut policies = vec![current];
        loop {
            let policy = policies.last().expect("current policy is present");
            // Removing entries both bounds traversal by the retained history and
            // rejects cycles. Do not order opaque change IDs or use timestamps.
            let (previous, saved) = changes
                .remove(&policy.change_id)
                .ok_or_else(|| denied("Subject policy continuity history is incomplete"))?;
            if saved.as_ref() != Some(policy) {
                return Err(denied("Subject policy continuity evidence does not match"));
            }
            let previous = previous
                .ok_or_else(|| denied("Subject policy continuity does not reach issuance"))?;
            if previous == source_change_id {
                return Ok(policies);
            }
            let prior = changes
                .get(&previous)
                .and_then(|(_, policy)| policy.as_ref())
                .filter(|policy| policy.change_id == previous)
                .ok_or_else(|| {
                    denied("Subject policy was removed or continuity history is missing")
                })?;
            policies.push(prior.clone());
        }
    }
}
