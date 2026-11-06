//! Short synchronous service scopes retain Feature, catalog and Host authority.
//! Nested checks use independent query-only connections while the original native
//! transactions and original registry lease remain held through Host commit.
use super::*;

pub(super) fn with_service_authority<A, T>(
    api: &WorkspaceApi,
    check: impl FnOnce(&WorkspaceApi) -> crate::Result<A>,
    effect: impl FnOnce(&rusqlite::Transaction<'_>, A) -> crate::Result<T>,
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
            let admitted = check(&rebound)?;
            effect(tx, admitted)
        })
    })
}
