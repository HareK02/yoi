//! Policy keys are declarations of the worker Feature, never automatic Set contributions.
use super::*;

pub(crate) fn sync_launch_policy_permissions(
    connection: &rusqlite::Connection,
    workspace: &str,
    registry: &mut worker::feature::permissions::FeaturePermissionRegistry,
) -> Result<()> {
    let namespace = feature_permissions::NamespaceId::parse("worker").map_err(Error::Config)?;
    let catalog = registry.host().catalog();
    let Some(existing) = catalog
        .namespaces
        .into_iter()
        .find(|entry| entry.namespace == namespace)
    else {
        return Err(Error::Config(
            "worker permission namespace is unavailable".into(),
        ));
    };
    let feature = worker::feature::FeatureId::new(existing.feature.as_str())
        .map_err(|e| Error::Config(e.to_string()))?;
    let handle = registry
        .namespace_handle(&feature, &namespace)
        .ok_or_else(|| Error::Config("worker Feature namespace handle is unavailable".into()))?
        .clone();
    let mut declaration = existing.declaration;
    declaration
        .permissions
        .retain(|permission| !permission.key.as_str().starts_with("worker.launch."));
    declaration
        .permissions
        .extend(crate::launch_policy::usage_declarations(
            &crate::launch_policy::policies(connection, workspace)?,
        )?);
    registry
        .update(&handle, declaration)
        .map_err(|e| Error::Config(e.to_string()))
}
