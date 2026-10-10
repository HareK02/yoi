// These are provenance and frozen replay evidence, not live update guards.
// Keep the original data while removing the misleading names from current DDL.
fn migrate_evidence_column_names_v88_to_v89(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 88 {
        return Err(Error::Store(
            "Evidence column migration requires schema 88".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("ALTER TABLE artifacts RENAME COLUMN source_revision TO source_reference;")?;
    rename_saved_secret_receipt_counters(&tx)?;
    tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES(89,'descriptive legacy evidence columns')", [])?;
    tx.commit()?;
    Ok(())
}

// Also used by focused legacy SSH fixtures that do not contain unrelated tables.
fn rename_saved_secret_receipt_counters(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE repository_secret_legacy_receipts RENAME COLUMN result_revision TO legacy_result_counter;
         ALTER TABLE repository_secret_legacy_receipts RENAME COLUMN expected_revision TO legacy_expected_counter;",
    )?;
    Ok(())
}
