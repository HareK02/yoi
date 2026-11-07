//! Short synchronous protected Feature scopes retain catalog and Host authority.
//! Nested checks use independent query-only connections while the original native
//! transactions and registry lease remain held through commit or result construction.
//! Callbacks must not call Runtime/network or reenter the original write owners.
use super::*;

/// Retain the verified request's trust identity across executor/lock waits.
/// Call on the reserved Host connection, not a detached current-binding lookup.
pub(super) fn require_authenticated_runtime_binding(
    conn: &rusqlite::Connection,
    workspace: &str,
    source: Option<&server_api::ServerRuntimeSource>,
) -> crate::Result<()> {
    let denied = || crate::plugin_host::failure(server_api::PluginHostErrorCode::Denied);
    let source = source.ok_or_else(denied)?;
    let current: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_runtime_bindings WHERE workspace_id=?1 AND runtime_id=?2 AND binding_id=?3 AND revoked_at IS NULL)",
        rusqlite::params![workspace, source.runtime_id, source.authentication_binding_id],
        |row| row.get(0),
    )?;
    if !current {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn with_service_authority<T>(
    api: &WorkspaceApi,
    effect: impl FnOnce(&WorkspaceApi, &rusqlite::Transaction<'_>) -> crate::Result<T>,
) -> crate::Result<T> {
    let feature = api.feature_storage.open(&api.subjektiv_registration)?;
    feature.try_transaction::<_, Error>(|_| {
        let mut registry = api
            .permission_registry
            .write()
            .map_err(|_| Error::Config("current Feature declarations unavailable".into()))?;
        api.config_store.with_immediate_transaction(|tx| {
            // BEGIN IMMEDIATE has now settled any preceding Host writer. Both
            // read owners observe that committed authority, not a preflight cache.
            let catalog = Arc::new(api.config_store.transaction_query_reader()?);
            let subjects = crate::subjektiv::SubjektivStore::from_database(feature.query_reader()?)
                .map_err(subjektiv_store_error)?;
            let root = ServerConfig::server_data_root_for_data_dir(&api.config.data_root)
                .join("plugin-artifacts");
            catalog.sync_plugin_permission_registry(&root, api.workspace_id(), &mut registry)?;
            let mut rebound = api.clone();
            rebound.store = catalog.clone();
            rebound.config_store = catalog;
            // The original registry is still exclusively fenced. Nested helpers
            // may refresh/read this coherent private view without recursive locks.
            rebound.permission_registry = Arc::new(RwLock::new(registry.clone()));
            rebound.subject_authority_store = Some(subjects);
            effect(&rebound, tx)
        })
    })
}
