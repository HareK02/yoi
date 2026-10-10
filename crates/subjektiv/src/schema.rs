//! Current schema creation. Frozen data conversion lives with the old migrations.
use super::*;

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    legacy_migrations::remove_state_numbers(tx)
}

pub(super) fn create(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(include_str!("schema.sql"))?;
    Ok(())
}
