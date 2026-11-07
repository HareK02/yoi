//! Delivery admission is bound to an authenticated Host identity and a persisted
//! exact plan pin, not management permission or possession of a digest. Service
//! adapters must derive the identity and recheck live Host authority on every
//! call, including cache hits; these database checks are additional constraints.
//!
//! Reads never change plan retention. Revoked and released plans remain audit
//! evidence, but cannot authorize metadata or bytes. Bytes are read while holding
//! the catalog's SQLite write fence, preventing revoke/uninstall/GC interleaving.
use crate::{
    Result,
    plugin_catalog::{self, PluginCatalogError},
    plugin_plan::{self, HostExecutionIdentity, PlanPinState},
    store::SqliteWorkspaceStore,
};
use rusqlite::{Transaction, TransactionBehavior, params};
use server_api::{PluginArtifactDeliveryMetadata, PluginArtifactDeliveryRequest};
use std::path::Path;

/// Promote fresh descriptor metadata to a cache expectation only after binding
/// every immutable identity to the saved Host pin. This does not admit use; the
/// cache must still ask the current owner on each resolution.
pub fn artifact_expectation(
    pin: &crate::plugin_plan::WorkerPluginPin,
    artifact: &server_api::PluginArtifact,
) -> Result<yoi_plugin_runtime::delivery::ArtifactExpectation> {
    if !pin.matches_artifact(artifact)? {
        return Err(PluginCatalogError::Conflict.into());
    }
    Ok(yoi_plugin_runtime::delivery::ArtifactExpectation::new(
        artifact.clone(),
    ))
}

/// The caller must hold an immediate transaction in the plan/catalog owner DB.
/// The current catalog operation is read inside this snapshot: unrelated version
/// additions must not invalidate the immutable pin's creation-time observation.
/// Current config is deliberately not resolved again, avoiding latest selection.
pub fn plugin_delivery_authorize(
    tx: &Transaction<'_>,
    workspace: &str,
    execution: &HostExecutionIdentity,
    request: &PluginArtifactDeliveryRequest,
) -> Result<PluginArtifactDeliveryMetadata> {
    let saved = plugin_plan::plugin_plan_load(tx, workspace, &request.plan_id)?;
    if saved.fingerprint != request.plan_fingerprint || saved.plan.execution() != execution {
        return Err(PluginCatalogError::Conflict.into());
    }
    let kind = match saved.pin_state {
        PlanPinState::Pending => "pending",
        PlanPinState::Active => "active",
        PlanPinState::Restorable => "restorable",
        PlanPinState::Released => return Err(PluginCatalogError::Conflict.into()),
    };
    let pin = saved
        .plan
        .plugins()
        .iter()
        .find(|pin| {
            pin.installation_id() == request.installation_id
                && pin.artifact_digest() == request.artifact_digest
        })
        .ok_or(PluginCatalogError::NotFound)?;
    let retained: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM plugin_references WHERE workspace_id=?1 AND kind=?2 AND owner=?3 AND installation_id=?4 AND artifact_digest=?5)",
        params![workspace, kind, request.plan_id, request.installation_id, request.artifact_digest],
        |row| row.get(0),
    )?;
    if !retained {
        return Err(PluginCatalogError::Conflict.into());
    }
    let operation: String = tx.query_row(
        "SELECT last_operation FROM plugin_installations WHERE workspace_id=?1 AND installation_id=?2",
        params![workspace, request.installation_id],
        |row| row.get(0),
    )?;
    let artifact = plugin_catalog::plugin_artifact_admit(
        tx,
        workspace,
        &request.installation_id,
        &operation,
        pin.package_digest(),
        &request.artifact_digest,
    )?;
    // Reconstruct using the *saved* projection and operation observation. This
    // compares all immutable identities, size, Component world/imports and schema
    // identity while retaining the original config/grants rather than live config.
    if !pin.matches_artifact(&artifact)? {
        return Err(PluginCatalogError::Conflict.into());
    }
    Ok(PluginArtifactDeliveryMetadata {
        workspace_id: workspace.into(),
        request: request.clone(),
        artifact,
    })
}

/// Reauthorizes in the same owner transaction as the bounded immutable blob read.
/// A previous metadata response (including a cache descriptor) is not authority.
pub fn plugin_delivery_bytes(
    tx: &Transaction<'_>,
    root: &Path,
    workspace: &str,
    execution: &HostExecutionIdentity,
    request: &PluginArtifactDeliveryRequest,
) -> Result<Vec<u8>> {
    let admitted = plugin_delivery_authorize(tx, workspace, execution, request)?;
    plugin_catalog::read_blob(
        root,
        &admitted.artifact.artifact_digest,
        u64::from(admitted.artifact.artifact_size),
    )
}

impl SqliteWorkspaceStore {
    pub fn plugin_delivery_authorize(
        &self,
        workspace: &str,
        execution: &HostExecutionIdentity,
        request: &PluginArtifactDeliveryRequest,
    ) -> Result<PluginArtifactDeliveryMetadata> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let metadata = plugin_delivery_authorize(&tx, workspace, execution, request)?;
            tx.commit()?;
            Ok(metadata)
        })
    }

    pub fn plugin_delivery_bytes(
        &self,
        root: &Path,
        workspace: &str,
        execution: &HostExecutionIdentity,
        request: &PluginArtifactDeliveryRequest,
    ) -> Result<Vec<u8>> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let bytes = plugin_delivery_bytes(&tx, root, workspace, execution, request)?;
            tx.commit()?;
            Ok(bytes)
        })
    }
}

#[cfg(test)]
mod tests;
