//! Representation-only migration of current canonical source and policy rows.
//!
//! Historical authority, receipts, revocations and frozen transfers are evidence,
//! not migration targets. A failed equivalence proof or write rolls back the whole
//! Server transaction; no owner edit or policy change identity is manufactured.

use super::*;
use config_source::{EvaluationResult, SnapshotEnvironment};

const ACTOR: &str = "system:permission-reference-migration";

/// Run after automatic storage adoption (including its canonical no-op), before
/// making this Workspace available. No native Feature store is read or modified.
pub(crate) fn migrate(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    schema: WorkspaceConfigSchemaBundle,
    host: &PermissionHost,
) -> Result<bool> {
    if host.catalog().workspace.as_str() != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    store.with_immediate_transaction(|tx| {
        if !canonical(tx, workspace)? {
            return Ok(false);
        }
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND state='active')",
            [workspace], |row| row.get(0),
        )?;
        if !active {
            return Err(Error::WorkspaceIdMismatch);
        }
        let original = crate::config_source::load_state(tx, workspace)?.ok_or_else(|| {
            Error::RegistryInconsistency("canonical Workspace source is not materialized".into())
        })?;
        let before = SnapshotEnvironment::new(original.snapshot.clone())
            .evaluate_contract(&original.contract)
            .map_err(|error| Error::InvalidInput(format!("cannot evaluate current reference migration source: {error:?}")))?;
        let mut source = crate::configuration_migration::shared_source_for_migration(&original, &before)?;
        let changes = crate::configuration_migration::shared_source_migration_changes(&original, &mut source)?;
        let candidate = crate::configuration_migration::evaluate_migration_candidate(
            tx, workspace, &original, &changes, schema,
        )?;
        prove_reference_only(&original, &before, &candidate.evaluation)?;
        crate::config_source::validate_workspace_config_candidate_projections(workspace, &candidate)?;
        crate::repository_access::validate_repository_access_in_transaction(tx, workspace, &candidate)?;
        let evaluated_source = policy_config::source(&candidate.evaluation)?;
        if evaluated_source != source {
            return Err(Error::InvalidInput("reference migration changed the shared permission source beyond the exact mapping".into()));
        }
        policy_config::validate_source(host, &evaluated_source, |_| Ok(()))?;
        let configured = policy_config::configured_host(host, &evaluated_source)?;
        // Only current domain rows are authority. Never apply, replay or reconstruct
        // a Subject policy from configuration, including an old explicit removal.
        let mut current_policies = policies(tx, workspace)?;
        let mut policies_changed = false;
        for policy in current_policies.values_mut() {
            policies_changed |= policy.migrate_reference_encoding();
            policy_config::validate_assignment(&configured, &policy.source.execution)?;
            policy_config::validate_assignment(&configured, &policy.source.delegation)?;
        }
        let state = WorkspaceConfigState {
            snapshot: candidate.snapshot,
            contract: candidate.contract,
            projection_digest: candidate.evaluation.projection_digest,
        };
        if state == original && !policies_changed {
            return Ok(false);
        }
        crate::config_source::sync_adopted_config_references(tx, workspace, &state)?;
        let authority = OwnerScope(workspace);
        let content = ScopedStore::new(tx, scope(workspace), &authority);
        let prepared = content_history_adapters::settings::prepare(&state.snapshot).map_err(content_error)?;
        for (id, object) in &prepared.objects {
            content.put(*id, &object.encode()).map_err(content_error)?;
        }
        tx.execute("DELETE FROM workspace_config_entries WHERE workspace_id=?1", [workspace])?;
        crate::config_source::insert_materialized_state(tx, workspace, &state, &chrono::Utc::now().to_rfc3339())?;
        for (subject, policy) in &current_policies {
            tx.execute(
                "UPDATE workspace_configuration_policies SET policy_json=?3 WHERE workspace_id=?1 AND subject_id=?2",
                params![workspace, subject, json(policy)?],
            )?;
        }
        content.adopt(prepared.root, head(tx, workspace)?, ACTOR.into(), chrono::Utc::now().timestamp_micros())
            .map_err(content_error)?;
        Ok(true)
    })
}

pub(crate) fn prove_reference_only(
    original: &WorkspaceConfigState,
    before: &EvaluationResult,
    after: &EvaluationResult,
) -> Result<()> {
    let mut expected = before.clone();
    let mut actual = after.clone();
    for projection in &mut expected.projections {
        // The stored contract selects old decoding; current input has no alias.
        let mut single = before.clone();
        single
            .projections
            .retain(|p| p.entrypoint == projection.entrypoint);
        let mut source =
            crate::configuration_migration::shared_source_for_migration(original, &single)?;
        policy_config::migrate_source_references(&mut source);
        if let Some(root) = projection.data_json.as_object_mut() {
            root.insert(
                "permissions".into(),
                serde_json::to_value(source).map_err(|error| Error::Store(error.to_string()))?,
            );
        }
    }
    for projection in &mut actual.projections {
        let mut single = after.clone();
        single
            .projections
            .retain(|p| p.entrypoint == projection.entrypoint);
        // Normalize schema materialization only through the strict current type.
        // Unknown fields, the old spelling and explicit empty assignments are not
        // discarded; all non-permission JSON remains part of the exact proof.
        let source = policy_config::source(&single)?;
        if let Some(root) = projection.data_json.as_object_mut() {
            root.insert(
                "permissions".into(),
                serde_json::to_value(source).map_err(|error| Error::Store(error.to_string()))?,
            );
        }
    }
    if expected
        .projections
        .iter()
        .map(|p| (&p.entrypoint, &p.data_json))
        .collect::<Vec<_>>()
        != actual
            .projections
            .iter()
            .map(|p| (&p.entrypoint, &p.data_json))
            .collect::<Vec<_>>()
    {
        return Err(Error::InvalidInput("cannot prove reference-only source migration; Workspace requires explicit reconciliation".into()));
    }
    Ok(())
}
