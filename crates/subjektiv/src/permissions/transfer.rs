//! One-way preparation of a permission-domain transfer. No Server adoption or dual commit.
use super::*;
use std::sync::Arc;

/// Host connection to the canonical permission domain, not an ACL or grant issuer.
/// Implementations must resolve the exact Workspace/Subject pair and propagate failures.
/// Calls are synchronous under the FeatureDatabase guard; do not reenter this store.
pub trait CanonicalPermissionSource: Send + Sync {
    fn subject_permissions(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Option<SubjectPermissions>>;
}

/// Serializable transfer DTO for FeatureStorage's typed identity, never a physical path.
/// FeatureStorage owns identity validation; this DTO does not interpret storage metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseIdentity {
    pub scope_id: String,
    pub feature_id: String,
    pub physical_id: String,
}

impl From<&feature_storage::DatabaseIdentity> for DatabaseIdentity {
    fn from(identity: &feature_storage::DatabaseIdentity) -> Self {
        Self {
            scope_id: identity.scope.clone(),
            feature_id: identity.feature.clone(),
            physical_id: identity.physical.clone(),
        }
    }
}

impl DatabaseIdentity {
    fn storage_identity(&self) -> feature_storage::DatabaseIdentity {
        feature_storage::DatabaseIdentity {
            scope: self.scope_id.clone(),
            feature: self.feature_id.clone(),
            physical: self.physical_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSourceExport {
    pub subject_id: String,
    /// `None` is unset; `Some` with empty vectors is an explicit revocation.
    pub permissions: Option<SubjectPermissions>,
}

/// Complete, Subject-ID-sorted policy snapshot. History and grants stay in the old database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionTransferSnapshot {
    pub identity: DatabaseIdentity,
    pub sources: Vec<PermissionSourceExport>,
    /// SHA-256 of the versioned identity and sorted policy vector, excluding this field.
    pub export_digest: String,
}

/// Immutable preparation evidence, not evidence that Server settings have been adopted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionTransferReceipt {
    pub transfer_id: String,
    pub snapshot: PermissionTransferSnapshot,
}

impl SubjektivStore {
    pub fn with_canonical_permission_source(mut self, source: Arc<dyn CanonicalPermissionSource>) -> Self {
        self.canonical_permission_source = Some(source);
        self
    }

    /// Reads physical identity under the FeatureStorage connection guard and scope check.
    pub fn database_identity(&self) -> Result<DatabaseIdentity> {
        let identity = DatabaseIdentity::from(self.database.identity());
        self.permission_transfer_transaction(&identity, |_| Ok(identity.clone()))
    }

    /// Reads one coherent pre-seal snapshot without installing schema or changing authority.
    /// After a transfer, returns the retained exact snapshot, never a changed/deleted live set.
    pub fn preview_permission_transfer(&self, expected: &DatabaseIdentity) -> Result<PermissionTransferSnapshot> {
        self.permission_transfer_transaction(expected, |tx| {
            if let Some(receipt) = retained_receipt(tx)? {
                require_receipt_identity(&receipt, expected)?;
                return Ok(receipt.snapshot);
            }
            require_unsealed(tx)?;
            export_in_connection(tx, expected.clone())
        })
    }

    /// Permanently seals the old source writer after preview/CAS. All changes, the exact export
    /// and its identity-bound receipt commit in one old-DB IMMEDIATE transaction. The Host must
    /// authorize/preview first and bind transfer_id to the complete Server request fingerprint.
    /// A successful seal does not adopt any Server configuration. Never call two commits atomic.
    pub fn seal_permission_sources(
        &self,
        expected: &DatabaseIdentity,
        transfer_id: &str,
        expected_export_digest: &str,
    ) -> Result<PermissionTransferReceipt> {
        validate_authorization_identifier(transfer_id).map_err(SubjektivError::InvalidRecord)?;
        self.permission_transfer_transaction(expected, |tx| {
            if let Some(receipt) = retained_receipt(tx)? {
                require_receipt_identity(&receipt, expected)?;
                if receipt.transfer_id != transfer_id || receipt.snapshot.export_digest != expected_export_digest {
                    return Err(SubjektivError::PermissionTransferConflict);
                }
                return Ok(receipt);
            }
            // A published Permission18 seal is not ours to rewrite or relabel as this receipt.
            require_unsealed(tx)?;
            let snapshot = export_in_connection(tx, expected.clone())?;
            if snapshot.export_digest != expected_export_digest {
                return Err(SubjektivError::PermissionTransferConflict);
            }
            crate::storage_formats::install_permission_transfer(tx)?;
            if !has_table(tx, "permission_source_transfer_receipt")? {
                return Err(SubjektivError::InvalidRecord("permission transfer requires schema 1..17".into()));
            }
            let receipt = PermissionTransferReceipt { transfer_id: transfer_id.into(), snapshot };
            let raw = serde_json::to_string(&receipt)?;
            tx.execute("INSERT INTO permission_authority_seal(singleton,transfer_id,export_json) VALUES(1,?1,?2)",
                params![transfer_id, serde_json::to_string(&receipt.snapshot)?])?;
            tx.execute("INSERT INTO permission_source_transfer_receipt(singleton,schema_version,receipt_json) VALUES(1,1,?1)", [&raw])?;
            Ok(receipt)
        })
    }

    fn permission_transfer_transaction<T>(
        &self,
        expected: &DatabaseIdentity,
        action: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        if *expected != DatabaseIdentity::from(self.database.identity()) {
            return Err(SubjektivError::PermissionTransferConflict);
        }
        // Validate the caller's native expected identity without holding a separate guard
        // across transaction entry. try_transaction rechecks the same immutable bound identity
        // before/after the operation and owns commit, including OutcomeUnknown errors.
        drop(self.database.guard(&expected.storage_identity())?);
        self.database.try_transaction(|tx| {
            require_store_scope(tx, self.scope_id())?;
            action(tx)
        })
    }

    pub(super) fn current_permission_source(&self, connection: &Connection, subject: &str) -> Result<Option<SubjectPermissions>> {
        if !is_sealed(connection)? {
            return source_in_connection(connection, subject);
        }
        let provider = self.canonical_permission_source.as_ref()
            .ok_or_else(|| denied("legacy permission authority is sealed; canonical permission source is not connected"))?;
        let source = provider.subject_permissions(self.scope_id(), subject)?;
        if let Some(source) = &source {
            validate_authorization_identifier(&source.change_id).map_err(SubjektivError::InvalidRecord)?;
            validate_scope(self.scope_id(), &source.execution)?;
            validate_scope(self.scope_id(), &source.delegation)?;
        }
        Ok(source)
    }
}

pub(crate) fn has_table(connection: &Connection, name: &str) -> Result<bool> {
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)", [name], |row| row.get(0))?)
}

pub(crate) fn is_sealed(connection: &Connection) -> Result<bool> {
    if !has_table(connection, "permission_authority_seal")? { return Ok(false); }
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM permission_authority_seal)", [], |row| row.get(0))?)
}

pub(super) fn require_unsealed(connection: &Connection) -> Result<()> {
    if is_sealed(connection)? { return Err(SubjektivError::PermissionAuthoritySealed); }
    Ok(())
}

fn require_store_scope(connection: &Connection, scope: &str) -> Result<()> {
    let stored: String = connection.query_row(
        "SELECT workspace_id FROM store_scope WHERE singleton=1",
        [], |row| row.get(0),
    )?;
    if stored != scope { return Err(SubjektivError::PermissionTransferConflict); }
    Ok(())
}

fn require_receipt_identity(receipt: &PermissionTransferReceipt, expected: &DatabaseIdentity) -> Result<()> {
    if receipt.snapshot.identity != *expected { return Err(SubjektivError::PermissionTransferConflict); }
    Ok(())
}

fn retained_receipt(connection: &Connection) -> Result<Option<PermissionTransferReceipt>> {
    if !has_table(connection, "permission_source_transfer_receipt")? { return Ok(None); }
    let raw: Option<String> = connection.query_row(
        "SELECT receipt_json FROM permission_source_transfer_receipt WHERE singleton=1 AND schema_version=1",
        [], |row| row.get(0),
    ).optional()?;
    let receipt: Option<PermissionTransferReceipt> = raw.map(|raw| serde_json::from_str(&raw)).transpose()?;
    if receipt.is_some() && !is_sealed(connection)? { return Err(SubjektivError::PermissionTransferConflict); }
    Ok(receipt)
}

fn export_in_connection(connection: &Connection, identity: DatabaseIdentity) -> Result<PermissionTransferSnapshot> {
    let orphaned: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM subject_permissions p LEFT JOIN subjects s ON s.subject_id=p.subject_id WHERE s.subject_id IS NULL)",
        [], |row| row.get(0),
    )?;
    if orphaned { return Err(SubjektivError::PermissionTransferConflict); }
    let mut statement = connection.prepare(
        "SELECT s.subject_id,p.change_id,c.record_json FROM subjects s
         LEFT JOIN subject_permissions p ON p.subject_id=s.subject_id
         LEFT JOIN subject_permission_changes c ON c.subject_id=p.subject_id AND c.change_id=p.change_id
         ORDER BY s.subject_id",
    )?;
    let mut rows = statement.query([])?;
    let mut sources = Vec::new();
    while let Some(row) = rows.next()? {
        let subject_id: String = row.get(0)?;
        validate_authorization_identifier(&subject_id).map_err(SubjektivError::InvalidRecord)?;
        let change: Option<String> = row.get(1)?;
        let raw: Option<String> = row.get(2)?;
        let permissions: Option<SubjectPermissions> = raw.map(|raw| serde_json::from_str(&raw)).transpose()?;
        if permissions.as_ref().map(|source| &source.change_id) != change.as_ref() {
            return Err(SubjektivError::PermissionTransferConflict);
        }
        if let Some(source) = &permissions {
            validate_authorization_identifier(&source.change_id).map_err(SubjektivError::InvalidRecord)?;
            validate_scope(&identity.scope_id, &source.execution)?;
            validate_scope(&identity.scope_id, &source.delegation)?;
        }
        sources.push(PermissionSourceExport { subject_id, permissions });
    }
    let export_digest = Sha256::digest(serde_json::to_vec(&(1_u32, &identity, &sources))?)
        .iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(PermissionTransferSnapshot { identity, sources, export_digest })
}

#[cfg(test)]
mod tests;
