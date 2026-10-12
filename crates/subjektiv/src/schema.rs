//! Creates the current schema and connects frozen migrations.
//!
//! Foreign keys for Memory history and candidate/derivation/surface references include the Subject,
//! rejecting cross-Subject references. Seals/triggers prevent adding references after immutable JSON and edges commit;
//! retention triggers reject deletion of history and referenced records. The repository updates current projections and search columns together.
use super::*;

pub(super) fn migrate(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    legacy_migrations::remove_state_numbers(tx)
}

pub(super) fn create(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    tx.execute_batch(include_str!("schema.sql"))?;
    Ok(())
}
