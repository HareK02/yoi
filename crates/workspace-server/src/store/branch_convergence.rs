//! Preserve both published 142/143 histories without relabeling their receipts.
//! Shape acceptance stays with storage_formats; this owner installs only the
//! missing branch inside one transaction, including the Memory retirement.
use super::*;

pub(super) const HARE_NAME: &str = "Hare membership and Feature launch schema convergence";
pub(super) const DEVELOP_NAME: &str = "Feature launch and membership schema convergence";
pub(super) const HARE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 142,
        name: passkey_addition_code::MIGRATION_NAME,
        apply: passkey_addition_code::migrate,
    },
    Migration {
        version: 143,
        name: workspace_members::MIGRATION_NAME,
        apply: workspace_members::migrate,
    },
    Migration {
        version: 145,
        name: HARE_NAME,
        apply: migrate,
    },
];

// This selects the expected exact history, NOT permission to accept a shape.
// After convergence the distinct receipt retains the identity of a fresh hare
// 142/143 baseline even though its physical shape has become the union.
pub(super) fn hare_history(conn: &Connection, current: i64) -> Result<bool> {
    let rows = workspace_schema_migration_history(conn)?;
    if let Some((_, name)) = rows.iter().find(|(v, _)| *v == 145) {
        if name == HARE_NAME {
            if rows.first().is_some_and(|(v, _)| *v >= 144) {
                return Err(Error::Store(
                    "Hare convergence has no published schema-144 baseline".into(),
                ));
            }
            return Ok(true);
        }
        if name == DEVELOP_NAME || name == SCHEMA_BASELINE_NAME {
            return Ok(false);
        }
    }
    if rows.iter().any(|(v, n)| {
        (*v == 142 && n == passkey_addition_code::MIGRATION_NAME)
            || (*v == 143 && n == workspace_members::MIGRATION_NAME)
    }) {
        return Ok(true);
    }
    if (142..=143).contains(&current)
        && rows
            .first()
            .is_some_and(|(v, n)| *v >= 142 && n == SCHEMA_BASELINE_NAME)
    {
        return table_exists(conn, "passkey_addition_codes");
    }
    Ok(false)
}

pub(super) fn tail(conn: &Connection, current: i64) -> Result<Vec<(i64, &'static str)>> {
    if hare_history(conn, current)? {
        Ok(vec![
            (142, passkey_addition_code::MIGRATION_NAME),
            (143, workspace_members::MIGRATION_NAME),
            (145, HARE_NAME),
        ])
    } else {
        Ok(vec![
            (142, feature_initialization::MIGRATION_NAME),
            (143, feature_worker_launch::MIGRATION_NAME),
            (144, memory_retirement::MIGRATION_NAME),
            (145, DEVELOP_NAME),
        ])
    }
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    let current = current_schema_version(conn)?;
    if !conn.is_autocommit() || !matches!(current, 143 | 144) {
        return Err(Error::Store(
            "Branch convergence requires published hare 143 or develop 144".into(),
        ));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Recheck exact receipts and physical programs under the write transaction.
    storage_formats::verify_history(&tx, current)?;
    let hare = hare_history(&tx, current)?;
    if hare && current == 143 {
        tx.execute_batch(feature_initialization::COLUMN_SCHEMA)?;
        tx.execute_batch(feature_worker_launch::SCHEMA)?;
        memory_retirement::install(&tx)?;
    } else if !hare && current == 144 {
        tx.execute_batch(include_str!("../passkey_addition_code_schema.sql"))?;
        tx.execute_batch(include_str!("../workspace_members_schema.sql"))?;
    } else {
        return Err(Error::Store(
            "Branch convergence predecessor family mismatch".into(),
        ));
    }
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(145,?1)",
        [if hare { HARE_NAME } else { DEVELOP_NAME }],
    )?;
    storage_formats::verify_history(&tx, 145)?;
    verify_workspace_resource_constraints(&tx)?;
    let violations: i64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "Branch convergence foreign-key violation".into(),
        ));
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
#[path = "branch_convergence_tests.rs"]
mod tests;
