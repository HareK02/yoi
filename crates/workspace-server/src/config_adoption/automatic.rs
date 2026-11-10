//! Internal, resumable storage migration; it never enrolls or reauthorizes a Subject.
//!
//! The frozen plan commits before the separate Feature seal. After sealing, retries
//! use that exact plan rather than current defaults or a newly generated owner edit.

use super::*;
use crate::configuration_migration::preview_migration;
use subjektiv::{PermissionTransferReceipt, PermissionTransferSnapshot, SubjektivStore};

pub(crate) const SCHEMA: &str = include_str!("automatic.sql");
pub(crate) const MIGRATION_NAME: &str = "Resumable internal permission storage migration";

pub(crate) fn blocked_reason(error: &Error) -> Option<&'static str> {
    match error {
        Error::WorkspaceConfigConflict(_) => {
            Some("configuration or pending transfer requires reconciliation")
        }
        Error::InvalidInput(_) => {
            Some("legacy permissions or explicit source require a preserving mapping")
        }
        Error::SubjektivSubjectNotFound(_) => {
            Some("Subject metadata is unavailable; the frozen transfer requires reconciliation")
        }
        Error::Configuration(failure) => match failure.details().reason {
            FailureReason::CatalogChanged => {
                Some("Feature catalog changed during the frozen migration")
            }
            FailureReason::EditingHeadChanged
            | FailureReason::SourceContentChanged
            | FailureReason::LegacyExportChanged => {
                Some("configuration changed during the frozen migration")
            }
            _ => None,
        },
        _ => None,
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    original: WorkspaceConfigState,
    target: WorkspaceConfigState,
    legacy: PermissionTransferSnapshot,
    policies: BTreeMap<String, AdoptedSubjectPolicy>,
    editing_head: Option<String>,
    catalog_digest: String,
}

impl Plan {
    fn transfer_id(&self, workspace: &str) -> Result<String> {
        let digest = config_source::digest_bytes(
            json(&("permission-storage-v1", workspace, self))?.as_bytes(),
        );
        Ok(format!(
            "permission-storage-v1-{}",
            digest.trim_start_matches("sha256:")
        ))
    }
}

pub(crate) fn pending(conn: &Connection, workspace: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_permission_migration_plans WHERE workspace_id=?1)",
        [workspace],
        |row| row.get(0),
    )?)
}

fn saved_plan(store: &SqliteWorkspaceStore, workspace: &str) -> Result<Option<Plan>> {
    store.with_conn(|conn| {
        let raw: Option<(String, String)> = conn.query_row(
            "SELECT transfer_id,plan_json FROM workspace_permission_migration_plans WHERE workspace_id=?1",
            [workspace], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        raw.map(|(id, raw)| {
            let plan: Plan = decode(&raw)?;
            if plan.transfer_id(workspace)? != id {
                return Err(Error::RegistryInconsistency("permission migration plan identity mismatch".into()));
            }
            Ok(plan)
        }).transpose()
    })
}

/// Runs under the native Feature lifecycle guard through the Server commit. A
/// blocked conversion leaves the old authority intact and does not gate ordinary
/// Worker creation. A post-seal failure retains the plan for exact recovery.
pub(crate) fn migrate(
    store: &SqliteWorkspaceStore,
    legacy: &SubjektivStore,
    workspace: &str,
    schema: WorkspaceConfigSchemaBundle,
    host: &feature_permissions::PermissionHost,
) -> Result<bool> {
    if host.catalog().workspace.as_str() != workspace || legacy.scope_id() != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    let identity = legacy.database_identity().map_err(legacy_error)?;
    let sealed = legacy
        .permission_transfer_receipt(&identity)
        .map_err(legacy_error)?;
    let adopted = store.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT transfer_identity,legacy_physical_identity,legacy_export_digest FROM workspace_configuration_authorities WHERE workspace_id=?1",
            [workspace], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        ).optional()?)
    })?;
    if let Some((transfer, physical, export)) = adopted {
        if !sealed.as_ref().is_some_and(|receipt| {
            receipt.transfer_id == transfer
                && receipt.snapshot.identity == identity
                && identity.physical_id == physical
                && receipt.snapshot.export_digest == export
        }) {
            return Err(Error::RegistryInconsistency(
                "adopted permission authority does not match the native Feature seal".into(),
            ));
        }
        return Ok(false);
    }
    let plan = match (saved_plan(store, workspace)?, sealed.as_ref()) {
        (Some(plan), receipt) => {
            if let Some(receipt) = receipt {
                if receipt.transfer_id != plan.transfer_id(workspace)?
                    || receipt.snapshot != plan.legacy
                {
                    return Err(Error::WorkspaceConfigConflict(
                        "permission seal does not match the saved internal migration".into(),
                    ));
                }
            }
            plan
        }
        (None, Some(_)) => {
            return Err(Error::WorkspaceConfigConflict(
                "pending owner permission transfer requires reconciliation of its original operation".into(),
            ));
        }
        (None, None) => {
            let seed = preview_migration(store, legacy, workspace, schema, host)?;
            let blocked = seed
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    matches!(
                        diagnostic,
                        policy_config::MigrationDiagnostic::ExplicitConfigurationRequired { .. }
                    )
                })
                .count();
            if blocked != 0 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "permission storage migration cannot preserve {blocked} legacy operations; explicit mapping is required"
                )));
            }
            Plan {
                original: seed.original,
                target: seed.state,
                legacy: seed.snapshot,
                policies: seed.policies,
                editing_head: store
                    .with_conn(|conn| Ok(head(conn, workspace)?.map(|id| id.to_string())))?,
                catalog_digest: catalog_digest(host)?,
            }
        }
    };
    if identity != plan.legacy.identity || identity.scope_id != workspace {
        return Err(Error::WorkspaceIdMismatch);
    }
    let transfer_id = plan.transfer_id(workspace)?;
    legacy.with_permission_authority_guard(&identity, |guard| {
        Ok((|| -> Result<bool> {
            // A competing initializer can finish while this operation is preparing.
            if store.with_conn(|conn| canonical(conn, workspace))? {
                return Ok(false);
            }
            if sealed.is_none() {
                guard.require_unsealed().map_err(legacy_error)?;
            }
            for source in &plan.legacy.sources {
                guard.require_subject(&source.subject_id).map_err(legacy_error)?;
            }
            store.with_immediate_transaction(|tx| {
                validate(tx, workspace, &plan, host)?;
                let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND state='active')", [workspace], |row| row.get(0))?;
                if !active { return Err(Error::WorkspaceConfigConflict("permission migration requires an active Workspace".into())); }
                let saved: Option<(String, String)> = tx.query_row(
                    "SELECT transfer_id,plan_json FROM workspace_permission_migration_plans WHERE workspace_id=?1",
                    [workspace], |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional()?;
                if let Some((id, raw)) = saved {
                    if id != transfer_id || decode::<Plan>(&raw)?.transfer_id(workspace)? != transfer_id {
                        return Err(Error::WorkspaceConfigConflict("a different frozen permission migration already exists".into()));
                    }
                } else {
                    tx.execute("INSERT INTO workspace_permission_migration_plans(workspace_id,transfer_id,plan_json) VALUES(?1,?2,?3)", params![workspace,transfer_id,json(&plan)?])?;
                }
                Ok(())
            })?;
            let receipt = guard.seal_permission_sources(&transfer_id, &plan.legacy.export_digest)
                .map_err(legacy_error)?;
            if receipt.snapshot != plan.legacy {
                return Err(Error::RegistryInconsistency("permission migration export changed".into()));
            }
            publish(store, workspace, &transfer_id, &plan, &receipt, host)?;
            Ok(true)
        })())
    }).map_err(legacy_error)?
}

fn publish(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    transfer_id: &str,
    plan: &Plan,
    receipt: &PermissionTransferReceipt,
    host: &PermissionHost,
) -> Result<()> {
    store.with_immediate_transaction(|tx| {
        validate(tx, workspace, plan, host)?;
        if canonical(tx, workspace)? {
            return Err(Error::WorkspaceConfigConflict(
                "permission authority changed during storage migration".into(),
            ));
        }
        tx.execute(
            "INSERT INTO workspace_configuration_authorities VALUES(?1,?2,?3,?4)",
            params![
                workspace,
                transfer_id,
                receipt.snapshot.identity.physical_id,
                receipt.snapshot.export_digest
            ],
        )?;
        // Preserve old change identities and resource constraints. Applying an
        // owner edit here would reissue policy and invalidate surviving grants.
        for (subject, policy) in &plan.policies {
            tx.execute(
                "INSERT INTO workspace_configuration_policies VALUES(?1,?2,?3)",
                params![workspace, subject, json(policy)?],
            )?;
        }
        crate::config_source::sync_adopted_config_references(tx, workspace, &plan.target)?;
        let authority = OwnerScope(workspace);
        let content = ScopedStore::new(tx, scope(workspace), &authority);
        let prepared = content_history_adapters::settings::prepare(&plan.target.snapshot)
            .map_err(content_error)?;
        for (id, object) in &prepared.objects {
            content.put(*id, &object.encode()).map_err(content_error)?;
        }
        tx.execute(
            "DELETE FROM workspace_config_entries WHERE workspace_id=?1",
            [workspace],
        )?;
        crate::config_source::insert_materialized_state(
            tx,
            workspace,
            &plan.target,
            &chrono::Utc::now().to_rfc3339(),
        )?;
        content
            .adopt(
                prepared.root,
                head(tx, workspace)?,
                "system:permission-storage-migration".into(),
                chrono::Utc::now().timestamp_micros(),
            )
            .map_err(content_error)?;
        tx.execute(
            "DELETE FROM workspace_permission_migration_plans WHERE workspace_id=?1",
            [workspace],
        )?;
        Ok(())
    })
}

fn validate(
    tx: &Transaction<'_>,
    workspace: &str,
    plan: &Plan,
    host: &PermissionHost,
) -> Result<()> {
    let baseline = ConfigurationMigrationBaseline {
        original: &plan.original,
        state: &plan.target,
        policies: &plan.policies,
        diagnostics: &[],
        export_digest: &plan.legacy.export_digest,
    };
    let request = ConfigurationAdoptionRequest {
        operation_id: plan.transfer_id(workspace)?,
        expected_head: plan.editing_head.clone(),
        expected_catalog_digest: plan.catalog_digest.clone(),
        expected_legacy_export: Some(plan.legacy.export_digest.clone()),
        source: ConfigCommitRequest {
            base_digest: plan.target.snapshot.digest.clone(),
            changes: vec![],
            entrypoints: plan.target.contract.entrypoints.clone(),
        },
    };
    // Native Subject existence was checked before seal while holding the same
    // Feature lifecycle guard. Do not reenter its committed transaction here.
    let mut require_subject = |subject: &str| {
        if plan
            .legacy
            .sources
            .iter()
            .any(|source| source.subject_id == subject)
        {
            Ok(())
        } else {
            Err(Error::SubjektivSubjectNotFound(subject.into()))
        }
    };
    let (_, state, _) = prepare(
        tx,
        workspace,
        &request,
        plan.target.contract.schema_bundle.clone(),
        host,
        Some(&baseline),
        &mut require_subject,
        FailureOperation::Adopt,
    )?;
    if state != plan.target {
        return Err(Error::WorkspaceConfigConflict(
            "frozen permission migration no longer evaluates identically".into(),
        ));
    }
    Ok(())
}

fn legacy_error(error: subjektiv::SubjektivError) -> Error {
    match error {
        subjektiv::SubjektivError::SubjectNotFound(subject) => {
            Error::SubjektivSubjectNotFound(subject)
        }
        other => Error::Store(format!("permission storage migration: {other}")),
    }
}
