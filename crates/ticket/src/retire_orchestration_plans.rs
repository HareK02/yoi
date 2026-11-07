//! Startup-only retirement of the old planning ledger. Every stored field is retained
//! as a historical comment on its owning Ticket, including unknown kinds and optional
//! fields. No old memo is interpreted as a current decision, relation, assignment,
//! approval or execution grant. The caller owns the migration transaction/savepoint;
//! a failed append or drop must leave the complete old ledger available for retry.
use rusqlite::{Connection, params};

use crate::{Result, sqlite_err};

#[cfg(test)]
#[path = "retire_orchestration_plans_tests.rs"]
mod tests;

pub(super) fn migrate(connection: &Connection) -> Result<()> {
    {
        // Do not use the former bounded public query: it skipped unknown kinds and
        // could not establish that every Workspace/Ticket row had been preserved.
        let mut statement = connection
            .prepare(
                "SELECT workspace_id, ticket_id, record_id, kind, related_ticket, note,
                    accepted_summary, accepted_branch, accepted_worktree, accepted_role_plan,
                    author, at
             FROM typed_ticket_orchestration_plans
             ORDER BY workspace_id, ticket_id, at, record_id",
            )
            .map_err(sqlite_err)?;
        let mut rows = statement.query([]).map_err(sqlite_err)?;
        while let Some(row) = rows.next().map_err(sqlite_err)? {
            let workspace: String = row.get(0).map_err(sqlite_err)?;
            let ticket: String = row.get(1).map_err(sqlite_err)?;
            let original_at: String = row.get(11).map_err(sqlite_err)?;
            let mut original = serde_json::Map::new();
            for (index, name) in [
                "workspace_id",
                "ticket_id",
                "record_id",
                "kind",
                "related_ticket",
                "note",
                "accepted_summary",
                "accepted_branch",
                "accepted_worktree",
                "accepted_role_plan",
                "author",
                "at",
            ]
            .iter()
            .enumerate()
            {
                let value: Option<String> = row.get(index).map_err(sqlite_err)?;
                original.insert(
                    (*name).into(),
                    serde_json::to_value(value).map_err(sqlite_err)?,
                );
            }
            let original = serde_json::to_string_pretty(&original).map_err(sqlite_err)?;
            let body = format!(
                "Historical Orchestration Plan memo (retired storage)\n\n\
                 This is preserved historical information, not a current assignment, approval, \
                 execution permission or dependency. Read current Ticket requirements, typed \
                 relations and Assignment before acting. Any still-relevant requirement or decision \
                 must be evaluated in that context; migration does not adopt it automatically.\n\n\
                 Original record (all fields, without reinterpretation):\n```json\n{original}\n```"
            );
            let index: i64 = connection
                .query_row(
                    "SELECT COALESCE(MAX(event_index), -1) + 1 FROM typed_ticket_events
                 WHERE workspace_id=?1 AND ticket_id=?2",
                    params![workspace, ticket],
                    |row| row.get(0),
                )
                .map_err(sqlite_err)?;
            connection.execute(
                "INSERT INTO typed_ticket_events
                 (workspace_id, ticket_id, event_index, kind, author, at, heading, body, event_ref, archived)
                 VALUES (?1, ?2, ?3, 'comment', 'migration:ticket-plan-retirement', ?4,
                         'Historical memo from retired storage', ?5, ?6, 0)",
                params![workspace, ticket, index, original_at, body, format!("{ticket}:{index}")],
            ).map_err(sqlite_err)?;
        }
    }
    connection
        .execute_batch("DROP TABLE typed_ticket_orchestration_plans;")
        .map_err(sqlite_err)
}
