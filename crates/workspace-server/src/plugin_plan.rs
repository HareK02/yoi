//! Immutable, Server-owned Plugin execution snapshots and same-database pins.
//!
//! The config owner supplies an already validated, non-secret projection; this
//! module never reads config, selects a Profile, fetches packages or resolves
//! credentials. Host-bound execution identities are trusted inputs, not caller
//! authentication. Service entrypoints must authenticate/authorize every call,
//! including replay and reads. A saved plan is an upper bound, not live authority.
//!
//! Owner transaction functions compose with Worker creation in the catalog DB.
//! Their local savepoints also roll back partial pin acquisition if an owner
//! catches an error and commits other work. There is no cross-store compensation.
use crate::{Error, Result, plugin_catalog::PluginCatalogError};
use serde::{Deserialize, Serialize};

pub use plugin_plan::{
    ArtifactDescriptor, ArtifactIdentityProjection, BindingPolicyReference, ConfigProvenance,
    HostExecutionIdentity, PlanError, PluginGrant, PluginPinProjection, PluginSchemaIdentity,
    PluginSurface, ProfileProvenance, WorkerPluginExecutionPlan, WorkerPluginPin,
    WorkerPluginPlanInput,
};

mod store;
pub use store::*;

/// Included by the config owner's ordered database migration, never at runtime.
pub const SCHEMA_SQL: &str = include_str!("plugin_plan/schema.sql");
pub(crate) const LAUNCH_SCHEMA_SQL: &str = include_str!("plugin_plan/launch_schema.sql");

impl From<PlanError> for Error {
    fn from(error: PlanError) -> Self {
        Self::PluginCatalog(match error {
            PlanError::Invalid => PluginCatalogError::Invalid,
            PlanError::Conflict => PluginCatalogError::Conflict,
        })
    }
}

fn invalid<T>() -> Result<T> {
    Err(Error::PluginCatalog(PluginCatalogError::Invalid))
}
fn conflict<T>() -> Result<T> {
    Err(Error::PluginCatalog(PluginCatalogError::Conflict))
}
fn not_found<T>() -> Result<T> {
    Err(Error::PluginCatalog(PluginCatalogError::NotFound))
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests;
