//! Retained physical metadata from published FeatureStorage writers. These are
//! provenance, not authority: adoption still needs a Host grant, attribution and
//! receipt/shape validation. Never rewrite their identity or provisioning receipt.
use super::*;

pub(super) fn identity(path: &Path, scope: &str, feature: &str) -> Result<Option<String>> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let exists = |name: &str| -> Result<bool> {
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
            [name],
            |r| r.get(0),
        )?)
    };
    if !exists("__yoi_feature_database_identity")? {
        if exists("__yoi_feature_provisioning_receipt")? {
            return Err(FeatureStorageError::Identity(
                "legacy provisioning receipt has no physical identity".into(),
            ));
        }
        return Ok(None);
    }
    verify_shape(
        &connection,
        "__yoi_feature_database_identity",
        "CREATE TABLE __yoi_feature_database_identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1), format INTEGER NOT NULL CHECK(format=1), scope_id TEXT NOT NULL,feature_id TEXT NOT NULL,incarnation TEXT NOT NULL)",
    )?;
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM __yoi_feature_database_identity",
        [],
        |r| r.get(0),
    )?;
    let (format,saved_scope,saved_feature,physical): (i64,String,String,String) = connection.query_row(
        "SELECT format,scope_id,feature_id,incarnation FROM __yoi_feature_database_identity WHERE singleton=1",[],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let id = Uuid::parse_str(&physical)
        .map_err(|_| FeatureStorageError::Identity("corrupt legacy physical identity".into()))?;
    if count != 1
        || format != 1
        || scope != saved_scope
        || feature != saved_feature
        || id.get_version_num() != 7
        || id.to_string() != physical
    {
        return Err(FeatureStorageError::Identity(
            "legacy physical attribution differs or is corrupt".into(),
        ));
    }
    if exists("__yoi_feature_provisioning_receipt")? {
        verify_shape(
            &connection,
            "__yoi_feature_provisioning_receipt",
            "CREATE TABLE __yoi_feature_provisioning_receipt (singleton INTEGER PRIMARY KEY CHECK(singleton=1),request_fingerprint TEXT NOT NULL)",
        )?;
        let (count,fingerprint): (i64,String) = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM __yoi_feature_provisioning_receipt),request_fingerprint FROM __yoi_feature_provisioning_receipt WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        if count != 1
            || fingerprint.len() != 71
            || !fingerprint.starts_with("sha256:")
            || !fingerprint.as_bytes()[7..]
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            return Err(FeatureStorageError::Identity(
                "corrupt legacy provisioning receipt".into(),
            ));
        }
    }
    Ok(Some(physical))
}

pub(super) fn fenced(root: &Path, scope: &str) -> Result<bool> {
    fenced_inner(root, scope, false)
}

// Only restore may inspect a lost live scope; it must still match snapshot
// provenance against completed new-owner receipts before admitting any bytes.
pub(super) fn fenced_for_restore(root: &Path, scope: &str) -> Result<bool> {
    fenced_inner(root, scope, true)
}

fn fenced_inner(root: &Path, scope: &str, recovering_loss: bool) -> Result<bool> {
    if root
        .join(".lifecycle")
        .join(format!("{scope}.deleted"))
        .try_exists()?
    {
        return Ok(true);
    }
    let record_path = root.join(".scope-locks").join(format!("{scope}.lock"));
    let record = match fs::read_to_string(record_path) {
        Ok(record) if !record.is_empty() => record,
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let fields: Vec<_> = record.trim_end_matches('\n').split(' ').collect();
    if fields.len() != 3 || fields[0] != "v1" {
        return Err(FeatureStorageError::Identity(
            "corrupt legacy scope record".into(),
        ));
    }
    let id = Uuid::parse_str(fields[2])
        .map_err(|_| FeatureStorageError::Identity("corrupt legacy scope identity".into()))?;
    if record != format!("v1 {} {id}\n", fields[1]) {
        return Err(FeatureStorageError::Identity(
            "noncanonical legacy scope record".into(),
        ));
    }
    match fields[1] {
        "deleted" | "deleting" => Ok(true),
        "live" => {
            let marker = fs::read_to_string(root.join(scope).join(".incarnation"));
            if recovering_loss
                && !root.join(scope).try_exists()?
                && marker
                    .as_ref()
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            {
                return Ok(false);
            }
            if marker? != format!("{id}\n") {
                return Err(FeatureStorageError::Identity(
                    "legacy scope incarnation differs".into(),
                ));
            }
            Ok(false)
        }
        _ => Err(FeatureStorageError::Identity(
            "unfinished legacy scope requires reconciliation by its owner".into(),
        )),
    }
}

/// Reconstruct only a previously retained live scope marker, never a deletion
/// record or a new identity. Called after matching completed snapshot provenance.
pub(super) fn restore_marker(root: &Path, scope: &str, temporary: &Path) -> Result<()> {
    if fenced_for_restore(root, scope)? {
        return Err(FeatureStorageError::ScopeMaintenance(scope.into()));
    }
    let record = match fs::read_to_string(root.join(".scope-locks").join(format!("{scope}.lock"))) {
        Ok(record) => record,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if let Some(id) = record
        .strip_prefix("v1 live ")
        .and_then(|s| s.strip_suffix('\n'))
    {
        let marker = temporary.join(".incarnation");
        fs::write(&marker, format!("{id}\n"))?;
        std::fs::File::open(marker)?.sync_all()?;
    }
    Ok(())
}

fn verify_shape(connection: &Connection, table: &str, expected: &str) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name=?1",
        [table],
        |r| r.get(0),
    )?;
    // These two frozen DDL definitions contain no SQL string literals. Whitespace
    // and token case are immaterial; constraints and column programs are not.
    let normalized = |s: &str| {
        s.chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_lowercase()
    };
    if normalized(&sql) != normalized(expected) {
        return Err(FeatureStorageError::Identity(
            "legacy physical metadata shape differs".into(),
        ));
    }
    Ok(())
}
