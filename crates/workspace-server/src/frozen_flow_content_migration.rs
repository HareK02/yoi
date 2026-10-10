// Frozen schema-84 conversion. Legacy ordinal names are confined to this migration.
fn migrate_flow_content_v84_to_v85(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 84 {
        return Err(Error::Store(
            "Flow content migration requires schema 84".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    if table_exists(&tx, "flow_source_revisions")? {
        // Validate every saved row before deduplicating or changing the schema.
        // Do not recompile: compiler changes must not rewrite historical meaning.
        {
            let mut statement = tx.prepare(
                "SELECT workspace_id, flow_id, revision, content, content_digest, definition_json
                 FROM flow_source_revisions",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let workspace_id: String = row.get(0)?;
                let flow_id: String = row.get(1)?;
                let legacy_counter: i64 = row.get(2)?;
                let content: String = row.get(3)?;
                let stored_digest: String = row.get(4)?;
                let definition_json: String = row.get(5)?;
                let identity = format!("{workspace_id}/{flow_id} legacy counter {legacy_counter}");
                let source_digest = format!(
                    "sha256:{}",
                    Sha256::digest(content.as_bytes())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                );
                if source_digest != stored_digest {
                    return Err(Error::Store(format!(
                        "Flow content migration found source digest mismatch for {identity}"
                    )));
                }
                let definition: CompiledFlowDefinition =
                    serde_json::from_str(&definition_json).map_err(|error| {
                        Error::Store(format!(
                            "Flow content migration found invalid stored definition for {identity}: {error}"
                        ))
                    })?;
                if definition.content_digest != stored_digest {
                    return Err(Error::Store(format!(
                        "Flow content migration found definition digest mismatch for {identity}"
                    )));
                }
            }
        }
        let conflicts: i64 = tx.query_row(
        "SELECT COUNT(*) FROM (SELECT 1 FROM flow_source_revisions GROUP BY workspace_id,flow_id,content_digest
            HAVING COUNT(DISTINCT content)>1 OR COUNT(DISTINCT definition_json)>1)", [], |row| row.get(0))?;
        if conflicts != 0 {
            return Err(Error::Store(
                "Flow content migration found inconsistent content identity".into(),
            ));
        }
        tx.execute_batch(
            r#"
        CREATE TABLE flow_source_contents (
            workspace_id TEXT NOT NULL,
            flow_id TEXT NOT NULL,
            content TEXT NOT NULL,
            content_digest TEXT NOT NULL,
            definition_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, flow_id, content_digest),
            FOREIGN KEY (workspace_id, flow_id)
                REFERENCES flow_sources(workspace_id, flow_id) ON DELETE CASCADE
        );
        INSERT INTO flow_source_contents
            (workspace_id, flow_id, content, content_digest, definition_json, created_at)
        SELECT workspace_id, flow_id, content, content_digest, definition_json, MIN(created_at)
        FROM flow_source_revisions
        GROUP BY workspace_id, flow_id, content_digest;
        DROP TABLE flow_source_revisions;
        ALTER TABLE flow_sources DROP COLUMN revision;
    "#,
        )?;
    }
    let missing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM flow_sources s WHERE NOT EXISTS (
             SELECT 1 FROM flow_source_contents c WHERE c.workspace_id=s.workspace_id
             AND c.flow_id=s.flow_id AND c.content_digest=s.content_digest
             AND c.content=s.content)",
        [],
        |row| row.get(0),
    )?;
    if missing != 0 {
        return Err(Error::Store(
            "Flow content migration found missing current content".into(),
        ));
    }
    tx.execute("INSERT INTO __yoi_schema_migrations(version,name) VALUES(85,'content-addressed Flow sources')", [])?;
    tx.commit()?;
    Ok(())
}
