//! Published Journal migration fixtures after the independent permission cutover.

use rusqlite::Connection;

pub(crate) fn install(conn: &Connection, frozen: &str) {
    conn.execute_batch(frozen).unwrap();
    // Permission migration support was retired independently of Journal history.
    // Model its completed cutover only for the empty configuration part of this
    // synthetic fixture; keep the published Journal schema and values unchanged.
    for table in [
        "workspace_configuration_policies",
        "workspace_configuration_receipts",
        "workspace_configuration_authorities",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
            0,
            "configuration cutover fixture must not discard policy or receipt data"
        );
        conn.execute_batch(&format!("DROP TABLE {table};")).unwrap();
    }
    conn.execute_batch(crate::config_adoption::SCHEMA).unwrap();
    conn.execute_batch(include_str!("fixtures/published_journal124_values.sql"))
        .unwrap();
}
