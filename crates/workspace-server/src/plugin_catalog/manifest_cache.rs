//! Parsing cache for immutable, hash-verified package bytes, never installation
//! authority. Every hit still reads and verifies the blob; callers separately join
//! current adopted pins and revocation state before consulting this projection.
use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(crate) struct ArtifactManifestCache {
    manifests: Mutex<BTreeMap<String, Arc<plugin::PluginPackageManifest>>>,
}

impl ArtifactManifestCache {
    pub(super) fn read(
        &self,
        root: &Path,
        digest: &str,
        size: u64,
    ) -> Result<Arc<plugin::PluginPackageManifest>> {
        let bytes = read_blob(root, digest, size)?;
        let mut manifests = self
            .manifests
            .lock()
            .map_err(|_| Error::PluginCatalog(PluginCatalogError::Unavailable))?;
        if let Some(manifest) = manifests.get(digest) {
            return Ok(manifest.clone());
        }
        let package = plugin::read_plugin_package_bytes(
            &bytes,
            PluginSourceKind::Explicit,
            &PluginPackageLimits::default(),
        )
        .map_err(|_| Error::PluginCatalog(PluginCatalogError::Invalid))?;
        let manifest = Arc::new(package.package.manifest);
        // The catalog is shared across Workspaces; bound this disposable parse
        // projection independently of installation or retained artifact counts.
        if manifests.len() >= 128 {
            manifests.clear();
        }
        manifests.insert(digest.into(), manifest.clone());
        Ok(manifest)
    }
}

impl SqliteWorkspaceStore {
    pub(crate) fn plugin_artifact_manifest(
        &self,
        root: &Path,
        workspace: &str,
        installation: &str,
        digest: &str,
    ) -> Result<std::sync::Arc<plugin::PluginPackageManifest>> {
        self.with_conn(|conn| {
            // This projection reads immutable bytes and a coherent revocation
            // snapshot; it neither writes SQL nor upgrades a reserved read owner.
            let tx = conn.unchecked_transaction()?;
            let current = load_installation(&tx, workspace, installation)?;
            if current.uninstalled {
                return fail(PluginCatalogError::NotFound);
            }
            let artifact = load_artifact(&tx, workspace, installation, digest)?;
            if artifact.revoked {
                return fail(PluginCatalogError::Conflict);
            }
            let manifest =
                self.plugin_manifest_cache
                    .read(root, digest, artifact.artifact_size.into())?;
            tx.commit()?;
            Ok(manifest)
        })
    }
}
