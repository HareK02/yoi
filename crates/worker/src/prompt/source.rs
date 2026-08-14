//! Immutable effective Prompt catalog carrier.
//!
//! Prompt sources are resolved and evaluated by Workspace config authority.
//! This type carries only the already-materialized projection into Worker
//! construction; it performs no filesystem, prefix, relative-path, user, or
//! repository discovery.

use std::sync::Arc;

use super::catalog::EffectivePromptCatalog;

#[derive(Debug, Clone, Default)]
pub struct PromptCatalogSource {
    effective_catalog: Option<Arc<EffectivePromptCatalog>>,
}

impl PromptCatalogSource {
    pub fn builtins_only() -> Self {
        Self::default()
    }

    pub fn with_effective_catalog(mut self, catalog: EffectivePromptCatalog) -> Self {
        self.effective_catalog = Some(Arc::new(catalog));
        self
    }

    pub fn effective_catalog(&self) -> Option<&EffectivePromptCatalog> {
        self.effective_catalog.as_deref()
    }
}
