//! Physical acceptance of retained ServerDB histories, not domain adoption.
//!
//! The store's ordered migration owner calls `verify_history` before writing.
//! Versions 50..113 still use its retained migrations (including both real 103
//! receipts). Published 114..121 databases retain their version, receipt names,
//! policy, Job states, root records and erasure fences. They need no domain DDL
//! merely to acquire the common storage identity/lifetime owned by the caller.
//! Ordered 122..129 receipts add configuration, Job execution, startup results,
//! bounded Job causes, Service outcomes, Journal removal, Plan retirement and independent
//! publication/connection observations; each intermediate/fresh baseline stays exact.
//! This does not attest policy transfer, execution termination or Journal release.
//!
//! Frozen SQL below is a physical reference: it runs ONLY on empty, private
//! in-memory databases. Never execute it on an adopted database. Later domain
//! migrations belong to their domain owners, not this recognizer. A successful
//! check is not a lock/reservation: the caller must hold the common storage
//! lifecycle guard through migration/adoption and consumer commit/rollback.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use super::{Connection, Error, Result};

const CONVERGED: &str = "Published schema tail convergence and canonical config source history";
const PRIOR_RELEASE: &str = "Worker removal original Subject release validation checkpoints";
const CLEANUP_COVERAGE: &str = "Backend cleanup complete invocation history origins";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Family {
    Permissions,
    Removal,
    Jobs,
}

const FAMILIES: [Family; 3] = [Family::Permissions, Family::Removal, Family::Jobs];

impl Family {
    fn names(self) -> &'static [&'static str] {
        match self {
            Self::Permissions => &[
                "Canonical config permission authority and sealed legacy transfer",
                "Durable installed Plugin permission registration lineage",
            ],
            Self::Removal => &[
                "Worker removal fixed Journal collection reference",
                "Worker removal expected Feature database identity",
                "Workspace creation Feature provisioning intents",
                "Immutable erased Worker removal supersession links",
            ],
            Self::Jobs => &[
                "Backend Job cleanup ownership claims",
                "Exact Journal cleanup child identities",
                "Backend Job live states and closed effect admission",
                "Backend Job actual cleanup invocation termination",
                "Backend cleanup kernel ownership",
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Base,
    Published(Family, usize),
    Converged(i64),
}

fn invalid(message: impl std::fmt::Display) -> Error {
    Error::Store(format!("Server storage format: {message}"))
}

/// Verify exact saved receipts AND physical tail shape without modifying `conn`.
/// Fresh baselines at a branch-local version are disambiguated by their shape.
/// Every other row, gap, mixed family, unknown suffix and missing object fails.
pub(super) fn verify_history(conn: &Connection, current: i64) -> Result<()> {
    let rows = super::workspace_schema_migration_history(conn)?;
    if current > super::LATEST_SCHEMA_VERSION {
        return Err(invalid(format!(
            "unknown baseline or current version: {rows:?}"
        )));
    }
    if current >= 128 {
        ticket::verify_sqlite_ticket_schema(conn).map_err(invalid)?;
    }
    verify_publication_shape(conn, current)?;
    verify_connection_observation_shape(conn, current)?;
    let tail = failure_tail(conn, current, &rows)?;
    verify_failure_shape(conn, current, tail)?;
    let journal = tail == FailureTail::Journal || current >= 127;
    if super::column_exists(conn, "worker_removal_operations", "journal_removal_json")? != journal {
        return Err(invalid("Journal removal receipt/shape mismatch"));
    }
    if journal {
        verify_journal_removal_shape(conn)?;
    }
    if current < 124 {
        return verify_history_records(conn, current, &rows);
    }
    verify_execution_shape(conn)?;
    super::backend_job_upgrade::verify(conn)?;
    let baseline = rows
        .first()
        .ok_or_else(|| invalid("missing migration receipts"))?;
    // A fresh published baseline has no fabricated predecessor receipts.
    if baseline.0 >= 124 {
        if baseline.1 != super::SCHEMA_BASELINE_NAME {
            return Err(invalid("unrecognized failure baseline"));
        }
        verify_configuration_shape(conn)?;
        require_shape(conn, Stage::Base)?;
        verify_103_features(conn, current, &rows)?;
    } else {
        let prefix: Vec<_> = rows.iter().filter(|row| row.0 < 124).cloned().collect();
        if prefix.last().map(|row| row.0) != Some(123) {
            return Err(invalid("missing failure migration predecessor"));
        }
        verify_history_records(conn, 123, &prefix)?;
    }
    let expected: Vec<_> = tail
        .migrations()
        .into_iter()
        .filter(|(version, _)| *version >= 124 && *version > baseline.0 && *version <= current)
        .map(|(version, name)| (version, name.to_owned()))
        .collect();
    let actual: Vec<_> = rows
        .iter()
        .filter(|row| row.0 >= 124 && row.0 > baseline.0)
        .cloned()
        .collect();
    if expected != actual || rows.last().map(|row| row.0) != Some(current) {
        return Err(invalid(
            "mixed, missing or unrecognized failure migration receipts",
        ));
    }
    Ok(())
}

fn verify_history_records(conn: &Connection, current: i64, rows: &[(i64, String)]) -> Result<()> {
    verify_raw_capture_shape(conn, current, rows)?;
    if current < 124
        && super::current_schema_version(conn)? < 124
        && super::column_exists(conn, "worker_removal_operations", "journal_removal_json")?
    {
        return Err(invalid("Journal removal scope before receipt 124"));
    }
    if current == 123 {
        verify_execution_shape(conn)?;
        super::backend_job_upgrade::verify(conn)?;
        if rows == [(current, super::SCHEMA_BASELINE_NAME.to_owned())] {
            verify_configuration_shape(conn)?;
            require_shape(conn, Stage::Base)?;
            return verify_103_features(conn, current, rows);
        }
        if rows.last().map(|(version, name)| (*version, name.as_str()))
            != Some((current, tail_migration_name(current)?))
        {
            return Err(invalid(format!("unrecognized schema-{current} receipt")));
        }
        let prefix = &rows[..rows.len() - 1];
        if prefix.last().map(|row| row.0) != Some(current - 1) {
            return Err(invalid(format!("missing schema-{current} predecessor")));
        }
        return verify_history_records(conn, current - 1, prefix);
    }
    if current == 122 {
        verify_configuration_shape(conn)?;
        if rows == [(122, super::SCHEMA_BASELINE_NAME.to_owned())] {
            require_shape(conn, Stage::Base)?;
            return verify_103_features(conn, current, rows);
        }
        if rows.last().map(|(version, name)| (*version, name.as_str()))
            != Some((122, crate::config_adoption::MIGRATION_NAME))
        {
            return Err(invalid("unrecognized configuration adoption receipt"));
        }
        let prefix = &rows[..rows.len() - 1];
        let prior = prefix
            .last()
            .map(|row| row.0)
            .ok_or_else(|| invalid("missing configuration predecessor"))?;
        if !(113..=121).contains(&prior) {
            return Err(invalid("invalid configuration predecessor"));
        }
        return verify_history_records(conn, prior, prefix);
    }
    let Some((baseline, name)) = rows.first() else {
        return Err(invalid("missing migration receipts"));
    };
    if !(50..=121).contains(baseline)
        || !(50..=121).contains(&current)
        || *baseline > current
        || name != super::SCHEMA_BASELINE_NAME
        || rows.last().map(|row| row.0) != Some(current)
    {
        return Err(invalid(format!(
            "unknown baseline or current version: {rows:?}"
        )));
    }
    let mut prefix = vec![(*baseline, super::SCHEMA_BASELINE_NAME.to_owned())];
    for migration in super::MIGRATIONS
        .iter()
        .filter(|migration| migration.version > *baseline && migration.version <= current.min(113))
    {
        let name = if migration.version == 103
            && rows.iter().any(|(version, name)| {
                *version == 103 && name == super::JOB_INPUT_RETENTION_SCHEMA_103.name
            }) {
            super::JOB_INPUT_RETENTION_SCHEMA_103.name
        } else {
            migration.name
        };
        prefix.push((migration.version, name.to_owned()));
    }
    if current <= 113 {
        if rows != prefix {
            return Err(invalid(format!(
                "unknown or missing retained receipt: {rows:?}"
            )));
        }
        reject_early_tail(conn, current)?;
        verify_103_features(conn, current, &rows)?;
        if current == 113 {
            require_shape(conn, Stage::Base)?;
        }
        return Ok(());
    }

    verify_103_features(conn, current, &rows)?;
    // A converged history may have jumped straight from 113, or retained any
    // exact published prefix. It must not acquire fabricated missing receipts.
    let prior = if current < 119 {
        current
    } else {
        rows.iter()
            .filter(|(version, _)| (114..=118).contains(version))
            .map(|row| row.0)
            .max()
            .unwrap_or((*baseline).max(113))
    };
    let stages: Vec<_> = if *baseline >= 119 {
        vec![Stage::Converged(current)]
    } else if prior == 113 {
        vec![Stage::Base]
    } else {
        FAMILIES
            .into_iter()
            .filter(|family| prior >= 114 && prior <= 113 + family.names().len() as i64)
            .map(|family| Stage::Published(family, (prior - 113) as usize))
            .collect()
    };
    let mut diagnostics = Vec::new();
    for stage in stages {
        let mut expected = prefix.clone();
        if let Stage::Published(family, count) = stage {
            expected.extend(
                family
                    .names()
                    .iter()
                    .take(count)
                    .enumerate()
                    .filter(|(index, _)| 114 + *index as i64 > *baseline)
                    .map(|(index, name)| (114 + index as i64, (*name).to_owned())),
            );
        }
        for (version, name) in [
            (119, CONVERGED),
            (120, PRIOR_RELEASE),
            (121, CLEANUP_COVERAGE),
        ] {
            if version > *baseline && version <= current {
                expected.push((version, name.to_owned()));
            }
        }
        if expected != rows {
            continue;
        }
        let installed = if current >= 119 {
            Stage::Converged(current)
        } else {
            stage
        };
        match require_shape(conn, installed) {
            Ok(()) => return Ok(()),
            Err(error) => diagnostics.push(error.to_string()),
        }
    }
    Err(invalid(format!(
        "unknown/mixed receipts or receipt-shape mismatch at {current}: {}",
        diagnostics.join("; ")
    )))
}

/// Plan physical acceptance of a published tail without downgrading it to 113
/// or claiming its policy/Job migrations have been performed by this binary.
/// The caller retains the existing migration path for all earlier versions.
pub(super) fn published_plan(
    conn: &Connection,
    current: i64,
) -> Result<Option<super::WorkspaceSchemaMigrationPlan>> {
    if current <= 113 {
        return Ok(None);
    }
    verify_history(conn, current)?;
    Ok(Some(super::WorkspaceSchemaMigrationPlan {
        current_schema_version: current,
        target_schema_version: super::LATEST_SCHEMA_VERSION,
        migrations: failure_tail(
            conn,
            current,
            &super::workspace_schema_migration_history(conn)?,
        )?
        .migrations()
        .into_iter()
        .filter(|(version, _)| *version > current)
        .map(|(version, name)| super::WorkspaceSchemaMigrationStep {
            version,
            name: name.to_owned(),
        })
        .collect(),
    }))
}

// Receipt names come from the ordered owner, including the Service-126 hook.
// A missing registration is an integration error, never a fabricated receipt.
fn tail_migration_name(version: i64) -> Result<&'static str> {
    super::MIGRATIONS
        .iter()
        .find(|migration| migration.version == version)
        .map(|migration| migration.name)
        .ok_or_else(|| invalid(format!("missing registered schema-{version} migration")))
}

// The two published histories share numbers, not identities. Fresh branch-local
// baselines are recognized by the exact physical shape checked below.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureTail {
    Canonical,
    Retained,
    Journal,
}
impl FailureTail {
    fn migrations(self) -> Vec<(i64, &'static str)> {
        let mut tail = vec![
            (122, crate::config_adoption::MIGRATION_NAME),
            (123, super::backend_job_upgrade::MIGRATION_NAME),
        ];
        tail.extend(match self {
            Self::Canonical => vec![
                (124, super::WORKER_CREATE_FAILURE_MIGRATION_NAME),
                (125, super::backend_job_recovery_store::CAUSE_MIGRATION_NAME),
                (126, super::SERVICE_OUTCOME_MIGRATION_NAME),
                (127, super::journal_removal_store::MIGRATION_NAME),
            ],
            Self::Retained => vec![
                (124, super::backend_job_recovery_store::CAUSE_MIGRATION_NAME),
                (125, super::SERVICE_OUTCOME_MIGRATION_NAME),
                (126, super::RETAINED_STARTUP_MIGRATION_NAME),
                (127, super::journal_removal_store::MIGRATION_NAME),
            ],
            Self::Journal => vec![
                (124, super::journal_removal_store::MIGRATION_NAME),
                (127, super::journal_removal_store::RETAINED_BRIDGE_NAME),
            ],
        });
        tail.push((128, super::TICKET_PLAN_RETIREMENT_MIGRATION_NAME));
        tail.push((129, super::DOMAIN_FAILURE_OBSERVATIONS_MIGRATION_NAME));
        tail
    }
    fn startup(self, current: i64) -> bool {
        current
            >= match self {
                Self::Canonical => 124,
                Self::Retained => 126,
                Self::Journal => 127,
            }
    }
    fn jobs(self, current: i64) -> bool {
        current
            >= match self {
                Self::Canonical => 125,
                Self::Retained => 124,
                Self::Journal => 127,
            }
    }
    fn service(self, current: i64) -> bool {
        current
            >= match self {
                Self::Canonical => 126,
                Self::Retained => 125,
                Self::Journal => 127,
            }
    }
}

fn failure_tail(conn: &Connection, current: i64, rows: &[(i64, String)]) -> Result<FailureTail> {
    if current > super::LATEST_SCHEMA_VERSION {
        return Err(invalid("unsupported future failure history"));
    }
    if current < 124 {
        return Ok(FailureTail::Canonical);
    }
    if let Some((_, name)) = rows
        .iter()
        .find(|row| row.0 == 124 && row.1 != super::SCHEMA_BASELINE_NAME)
    {
        return match name.as_str() {
            super::journal_removal_store::MIGRATION_NAME => Ok(FailureTail::Journal),
            super::WORKER_CREATE_FAILURE_MIGRATION_NAME => Ok(FailureTail::Canonical),
            super::backend_job_recovery_store::CAUSE_MIGRATION_NAME => Ok(FailureTail::Retained),
            _ => Err(invalid("unrecognized schema-124 identity")),
        };
    }
    let baseline = rows
        .first()
        .ok_or_else(|| invalid("missing failure baseline"))?
        .0;
    // The independently published Journal-124 baseline lacks all failure extensions.
    // Its complete shape and immutable receipt prefix are verified by the caller.
    if baseline == 124
        && !rows.iter().any(|row| (125..=126).contains(&row.0))
        && super::column_exists(conn, "worker_removal_operations", "journal_removal_json")?
    {
        return Ok(FailureTail::Journal);
    }
    // A retained baseline after completion is identified by its 126 receipt.
    if baseline < 126
        && rows
            .iter()
            .any(|row| row.0 == 126 && row.1 == super::RETAINED_STARTUP_MIGRATION_NAME)
    {
        return Ok(FailureTail::Retained);
    }
    if baseline == 124
        && rows
            .iter()
            .any(|row| row.0 == 125 && row.1 == super::SERVICE_OUTCOME_MIGRATION_NAME)
    {
        return Ok(FailureTail::Retained);
    }
    if current < 126
        && !super::column_exists(conn, "worker_create_reservations", "startup_failure_json")?
    {
        return Ok(FailureTail::Retained);
    }
    Ok(FailureTail::Canonical)
}

pub(super) fn verify_retained_journal_history(conn: &Connection) -> Result<()> {
    let current = super::current_schema_version(conn)?;
    let rows = super::workspace_schema_migration_history(conn)?;
    if current != 124 || failure_tail(conn, current, &rows)? != FailureTail::Journal {
        return Err(invalid(
            "Journal bridge requires the published Journal-124 history",
        ));
    }
    verify_history(conn, current)
}

// Compare tokenized complete table/index/trigger programs, not a substring that
// a comment or a second weaker constraint could counterfeit.
pub(super) fn verify_startup_shape(conn: &Connection) -> Result<()> {
    verify_startup_shape_for(conn, true)
}

fn verify_startup_shape_for(conn: &Connection, present: bool) -> Result<()> {
    static WITH_RESULT: OnceLock<std::result::Result<Shape, String>> = OnceLock::new();
    static WITHOUT_RESULT: OnceLock<std::result::Result<Shape, String>> = OnceLock::new();
    let select = |shape: Shape| -> Shape {
        shape
            .into_iter()
            .filter(|(_, object)| object.1 == "worker_create_reservations")
            .collect()
    };
    let expected = if present {
        &WITH_RESULT
    } else {
        &WITHOUT_RESULT
    }
    .get_or_init(|| {
        (|| -> Result<Shape> {
            let reference = reference_base()?;
            if present {
                reference.execute_batch(include_str!("worker_create_failure_schema.sql"))?;
            }
            Ok(select(snapshot(&reference)?))
        })()
        .map_err(|error| error.to_string())
    })
    .as_ref()
    .map_err(invalid)?;
    let actual = select(snapshot(conn)?);
    if actual == *expected {
        return Ok(());
    }
    // Real 53 -> 73 upgrades used the exact singleton triggers without the
    // redundant fresh-baseline table CHECK. Keep that exact published form,
    // including supported baseline-only histories that omit predecessor receipts.
    // The full table and both enforcing trigger programs still have to match.
    let history = super::workspace_schema_migration_history(conn)?;
    let retained_upgrade = history.first().is_some_and(|row| row.0 < 73)
        && history.iter().any(|row| {
            row.0 == 73
                && super::MIGRATIONS
                    .iter()
                    .any(|migration| migration.version == 73 && migration.name == row.1)
        });
    let retained_baseline = history.first().is_some_and(|row| {
        (73..=super::LATEST_SCHEMA_VERSION).contains(&row.0) && row.1 == super::SCHEMA_BASELINE_NAME
    });
    if retained_upgrade || retained_baseline {
        let mut upgraded = expected.clone();
        let (_, _, table) = upgraded
            .get_mut("worker_create_reservations")
            .ok_or_else(|| invalid("missing startup reference table"))?;
        let mut declaration =
            tokens("CHECK ((singleton_key IS NULL) = (singleton_generation IS NULL))")?;
        declaration.push(",".into());
        if let Some(start) = table
            .windows(declaration.len())
            .position(|part| part == declaration)
        {
            table.drain(start..start + declaration.len());
            if actual == upgraded {
                return Ok(());
            }
        }
    }
    let changed: BTreeSet<_> = actual
        .keys()
        .chain(expected.keys())
        .filter(|key| actual.get(*key) != expected.get(*key))
        .collect();
    Err(invalid(format!(
        "Worker create startup failure physical schema mismatch: {changed:?}"
    )))
}

// Run only for the saved version, not recursively against historical prefixes.
fn verify_failure_shape(conn: &Connection, current: i64, tail: FailureTail) -> Result<()> {
    let startup = super::column_exists(conn, "worker_create_reservations", "startup_failure_json")?;
    if startup != tail.startup(current) {
        return Err(invalid("startup failure receipt/shape mismatch"));
    }
    if current >= 124 {
        verify_startup_shape_for(conn, startup)?;
    }
    for (table, column) in [
        ("backend_jobs", "failure_cause"),
        ("backend_job_attempts", "failure_cause"),
        ("backend_job_attempts", "worker_cleanup_failure_cause"),
    ] {
        if super::column_exists(conn, table, column)? != tail.jobs(current) {
            return Err(invalid(format!("{table}.{column} receipt/shape mismatch")));
        }
    }
    if current < 112 {
        if super::column_exists(conn, "plugin_service_consumers", "last_outcome_json")? {
            return Err(invalid("Service outcome before its receipt"));
        }
        return Ok(());
    }
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(crate::plugin_service::SCHEMA_SQL)?;
    if tail.service(current) {
        reference.execute_batch(crate::plugin_service::FAILURE_MIGRATION_SQL)?;
    }
    let expected = snapshot(&reference)?;
    let tables: BTreeSet<_> = expected.values().map(|object| object.1.clone()).collect();
    let actual: Shape = snapshot(conn)?
        .into_iter()
        .filter(|(_, object)| tables.contains(&object.1))
        .collect();
    if actual != expected {
        return Err(invalid("Service outcome receipt/shape mismatch"));
    }
    Ok(())
}

fn verify_connection_observation_shape(conn: &Connection, current: i64) -> Result<()> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(crate::external_workdir_connection_observations::SCHEMA)?;
    let expected = snapshot(&reference)?;
    let tables: BTreeSet<_> = expected.values().map(|object| object.1.clone()).collect();
    let actual: Shape = snapshot(conn)?
        .into_iter()
        .filter(|(_, object)| tables.contains(&object.1))
        .collect();
    if (current >= 129 && actual != expected) || (current < 129 && !actual.is_empty()) {
        return Err(invalid(
            "External Workdir connection observation receipt/shape mismatch",
        ));
    }
    Ok(())
}

fn verify_publication_shape(conn: &Connection, current: i64) -> Result<()> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch(crate::config_publication::SCHEMA)?;
    let expected = snapshot(&reference)?;
    let tables: BTreeSet<_> = expected.values().map(|object| object.1.clone()).collect();
    let actual: Shape = snapshot(conn)?
        .into_iter()
        .filter(|(_, object)| tables.contains(&object.1))
        .collect();
    if (current >= 129 && actual != expected) || (current < 129 && !actual.is_empty()) {
        return Err(invalid("configuration publication receipt/shape mismatch"));
    }
    Ok(())
}

fn verify_configuration_shape(conn: &Connection) -> Result<()> {
    let expected_db = Connection::open_in_memory()?;
    expected_db.execute_batch(content_history_store::SCHEMA)?;
    expected_db.execute_batch(crate::config_adoption::SCHEMA)?;
    let expected = snapshot(&expected_db)?;
    let tables: BTreeSet<_> = expected.values().map(|object| object.1.clone()).collect();
    let actual: Shape = snapshot(conn)?
        .into_iter()
        .filter(|(_, object)| tables.contains(&object.1))
        .collect();
    if actual != expected {
        return Err(invalid("configuration adoption receipt/shape mismatch"));
    }
    Ok(())
}

// sqlite_schema programs, not table/column presence alone. SQL literal bytes,
// CHECK/FK clauses, indexes and trigger programs remain part of the contract.
type Shape = BTreeMap<String, (String, String, Vec<String>)>;
struct References {
    stages: BTreeMap<Stage, Shape>,
    tables: BTreeSet<String>,
    new_columns: BTreeMap<String, BTreeSet<String>>,
}

fn snapshot(conn: &Connection) -> Result<Shape> {
    let mut statement = conn.prepare("SELECT name,type,tbl_name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND name!='__yoi_schema_migrations' ORDER BY name")?;
    let mut shape = BTreeMap::new();
    for row in statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })? {
        let (name, kind, table, sql) = row?;
        let mut tokens = tokens(&sql)?;
        if kind == "table" {
            tokens = declarations(tokens)?;
        }
        shape.insert(name, (kind, table, tokens));
    }
    Ok(shape)
}

fn reference_base() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    super::configure_sqlite(&conn)?;
    // Frozen physical reference for published host schemas through 127.
    let tx = conn.unchecked_transaction()?;
    ticket::migrate_historical_sqlite_ticket_schema_in_transaction(
        &tx,
        ticket::HistoricalTicketSchemaTarget::ParentLinks16,
    )
    .map_err(invalid)?;
    tx.commit()?;
    merge_request::migrate(&conn).map_err(invalid)?;
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")?;
    let tx = conn.unchecked_transaction()?;
    super::create_workspace_schema_through_v113(&tx)?;
    tx.commit()?;
    Ok(conn)
}

fn references() -> Result<&'static References> {
    static REFERENCES: OnceLock<std::result::Result<References, String>> = OnceLock::new();
    REFERENCES
        .get_or_init(|| build_references().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(invalid)
}

fn build_references() -> Result<References> {
    let mut stages = BTreeMap::new();
    let mut columns = BTreeMap::new();
    let all = std::iter::once(Stage::Base)
        .chain(FAMILIES.into_iter().flat_map(|family| {
            (1..=family.names().len()).map(move |count| Stage::Published(family, count))
        }))
        .chain((119..=121).map(Stage::Converged));
    for stage in all {
        let conn = reference_base()?;
        install_reference(&conn, stage)?;
        let shape = snapshot(&conn)?;
        let mut fields = BTreeMap::new();
        for (name, (kind, _, _)) in &shape {
            if kind == "table" {
                fields.insert(
                    name.clone(),
                    super::table_columns(&conn, name)?
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                );
            }
        }
        columns.insert(stage, fields);
        stages.insert(stage, shape);
    }
    let base = &stages[&Stage::Base];
    let mut tables = BTreeSet::new();
    let mut new_columns: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (stage, shape) in &stages {
        for (name, object) in shape {
            if base.get(name) != Some(object) {
                tables.insert(object.1.clone());
            }
        }
        for (table, fields) in &columns[stage] {
            let old = columns[&Stage::Base]
                .get(table)
                .cloned()
                .unwrap_or_default();
            new_columns
                .entry(table.clone())
                .or_default()
                .extend(fields.difference(&old).cloned());
        }
    }
    Ok(References {
        stages,
        tables,
        new_columns,
    })
}

fn extension_shape(sql: &str) -> Result<Shape> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch("CREATE TABLE backend_jobs(workspace_id TEXT,job_id TEXT,request_json TEXT); CREATE TABLE backend_job_attempts(workspace_id TEXT,job_id TEXT,attempt_id TEXT); CREATE TABLE backend_job_deliveries(workspace_id TEXT,delivery_id TEXT);")?;
    reference.execute_batch(sql)?;
    let mut shape = snapshot(&reference)?;
    for name in [
        "backend_jobs",
        "backend_job_attempts",
        "backend_job_deliveries",
    ] {
        shape.remove(name);
    }
    Ok(shape)
}

fn require_extension(conn: &Connection, sql: &str) -> Result<Shape> {
    let expected = extension_shape(sql)?;
    let actual = snapshot(conn)?;
    let tables: BTreeSet<_> = expected
        .iter()
        .filter(|(_, object)| object.0 == "table")
        .map(|(_, object)| object.1.clone())
        .collect();
    for (name, object) in &actual {
        if tables.contains(&object.1) && !expected.contains_key(name) {
            return Err(invalid(format!("unreceipted Job evidence object: {name}")));
        }
    }
    for (name, object) in &expected {
        if actual.get(name) != Some(object) {
            return Err(invalid(format!("Job evidence schema drift: {name}")));
        }
    }
    Ok(expected)
}

fn verify_raw_capture_shape(conn: &Connection, current: i64, rows: &[(i64, String)]) -> Result<()> {
    let expected = extension_shape(super::backend_job_upgrade::RAW_SCHEMA)?;
    let actual = snapshot(conn)?;
    let mut execution = extension_shape(super::backend_job_upgrade::SCHEMA)?;
    execution.extend(extension_shape(super::backend_job_recovery_store::SCHEMA)?);
    if current < 123
        && super::current_schema_version(conn)? < 123
        && execution.keys().any(|name| actual.contains_key(name))
    {
        return Err(invalid("Backend Job execution objects before receipt 123"));
    }
    let present = expected.keys().any(|name| actual.contains_key(name));
    if !present {
        return Ok(());
    }
    if current < 86 {
        return Err(invalid("pre-content capture before its schema-86 boundary"));
    }
    require_extension(conn, super::backend_job_upgrade::RAW_SCHEMA)?;
    let populated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM backend_job_pre_content_history)",
        [],
        |row| row.get(0),
    )?;
    if populated
        && !(rows.first().is_some_and(|row| row.0 <= 85) && rows.iter().any(|row| row.0 == 86))
    {
        return Err(invalid(
            "populated pre-content capture without retained schema-86 history",
        ));
    }
    super::backend_job_upgrade::verify_pre_content(conn)
}

fn verify_execution_shape(conn: &Connection) -> Result<()> {
    require_extension(conn, super::backend_job_upgrade::RAW_SCHEMA)?;
    require_extension(conn, super::backend_job_upgrade::SCHEMA)?;
    require_extension(conn, super::backend_job_recovery_store::SCHEMA)?;
    Ok(())
}

// Ignore only separately validated evidence programs, not arbitrary extra Job
// triggers. The retained family's CHECK is transformed only after receipt 123;
// all other retained columns, programs and original receipt names remain exact.
fn legacy_shape(conn: &Connection) -> Result<Shape> {
    let mut shape = snapshot(conn)?;
    for sql in [
        super::backend_job_upgrade::RAW_SCHEMA,
        super::backend_job_upgrade::SCHEMA,
        super::backend_job_recovery_store::SCHEMA,
    ] {
        for name in extension_shape(sql)?.keys() {
            shape.remove(name);
        }
    }
    Ok(shape)
}

/// Validate the exact extension independently before interpreting predecessor
/// receipts. Reference SQL runs only in a private DB, never on adopted data.
fn journal_removal_extension() -> Result<(Shape, Vec<String>)> {
    let reference = Connection::open_in_memory()?;
    reference.execute_batch("CREATE TABLE worker_removal_operations(operation_id TEXT,plan_id TEXT,input_fingerprint TEXT,workspace_id TEXT,runtime_id TEXT,worker_id TEXT,state TEXT);")?;
    reference.execute_batch(super::journal_removal_store::SCHEMA_SQL)?;
    let mut shape = snapshot(&reference)?;
    let table = shape
        .remove("worker_removal_operations")
        .ok_or_else(|| invalid("missing Journal reference table"))?;
    let column = declaration(&table.2, "word:journal_removal_json")?;
    Ok((shape, column))
}

fn declaration(tokens: &[String], name: &str) -> Result<Vec<String>> {
    let start = tokens
        .iter()
        .position(|token| token == name)
        .ok_or_else(|| invalid("missing Journal removal declaration"))?;
    let mut depth = 0usize;
    for i in start..tokens.len() {
        match tokens[i].as_str() {
            "(" => depth += 1,
            ")" if depth == 0 => return Ok(tokens[start..i].to_vec()),
            ")" => depth -= 1,
            "," if depth == 0 => return Ok(tokens[start..i].to_vec()),
            _ => {}
        }
    }
    Err(invalid("unterminated Journal removal declaration"))
}

fn verify_journal_removal_shape(conn: &Connection) -> Result<()> {
    let (expected, column) = journal_removal_extension()?;
    let actual = snapshot(conn)?;
    if expected
        .iter()
        .any(|(name, object)| actual.get(name) != Some(object))
    {
        return Err(invalid(
            "Journal removal index/trigger receipt-shape mismatch",
        ));
    }
    let table = actual
        .get("worker_removal_operations")
        .ok_or_else(|| invalid("missing removal table"))?;
    if declaration(&table.2, "word:journal_removal_json")? != column {
        return Err(invalid(
            "Journal removal column/CHECK receipt-shape mismatch",
        ));
    }
    Ok(())
}

fn upgraded_reference(conn: &Connection, mut shape: Shape) -> Result<Shape> {
    let current = super::current_schema_version(conn)?;
    if current < 123 {
        return Ok(shape);
    }
    for table in ["backend_jobs", "backend_job_attempts"] {
        if let Some((_, _, tokens)) = shape.get_mut(table) {
            if table == "backend_job_attempts" {
                // This exact nullable declaration belongs only to receipt 123.
                // Canonical declarations are sorted; allocation precedes attempt.
                let start = tokens
                    .iter()
                    .position(|token| token == "(")
                    .ok_or_else(|| invalid("missing attempt declarations"))?;
                tokens.splice(
                    start + 1..start + 1,
                    [
                        "word:allocation_precluded_at".into(),
                        "word:text".into(),
                        ",".into(),
                    ],
                );
            }
            // These exact retained state declarations put the legacy value last.
            let start = tokens
                .windows(5)
                .position(|w| w == ["word:check", "(", "word:state", "word:in", "("]);
            if let Some(start) = start {
                if let Some(end) = tokens[start + 5..].iter().position(|token| token == ")") {
                    let end = start + 5 + end;
                    if end >= 2 && tokens[end - 1] == "literal:unknown" && tokens[end - 2] == "," {
                        tokens.drain(end - 2..end);
                    }
                }
            }
        }
    }
    if super::column_exists(conn, "backend_jobs", "failure_cause")? {
        for (table, column) in [
            ("backend_jobs", "failure_cause"),
            ("backend_job_attempts", "failure_cause"),
            ("backend_job_attempts", "worker_cleanup_failure_cause"),
        ] {
            let (_, _, table_tokens) = shape
                .get_mut(table)
                .ok_or_else(|| invalid(format!("missing reference table {table}")))?;
            append_reference_column(
                table_tokens,
                &format!(
                    "{column} TEXT CHECK({column} IS NULL OR length(CAST({column} AS BLOB))<=32768)"
                ),
            )?;
        }
    }
    if super::column_exists(conn, "worker_removal_operations", "journal_removal_json")? {
        let (extension, column) = journal_removal_extension()?;
        shape.extend(extension);
        if let Some((_, _, tokens)) = shape.get_mut("worker_removal_operations") {
            let start = tokens
                .iter()
                .position(|token| token == "(")
                .ok_or_else(|| invalid("missing removal declarations"))?;
            let mut addition = column;
            addition.push(",".into());
            tokens.splice(start + 1..start + 1, addition);
            *tokens = declarations(tokens.clone())?;
        }
    }
    Ok(shape)
}

// Canonical snapshots end every declaration with a comma. Reuse that synthetic
// separator before adding and re-sorting an exact declaration.
// This transforms private reference tokens only, never an adopted database.
fn append_reference_column(table: &mut Vec<String>, declaration: &str) -> Result<()> {
    let start = table
        .iter()
        .position(|token| token == "(")
        .ok_or_else(|| invalid("missing reference declarations"))?;
    let mut depth = 1;
    for i in start + 1..table.len() {
        match table[i].as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            let mut field = tokens(declaration)?;
            // Reuse the canonical last comma as the separator before the new
            // declaration; declarations() restores canonical trailing commas.
            if table.get(i - 1).map(String::as_str) != Some(",") {
                field.insert(0, ",".into());
            }
            table.splice(i..i, field);
            *table = declarations(table.clone())?;
            return Ok(());
        }
    }
    Err(invalid("unbalanced reference declarations"))
}

fn require_shape(conn: &Connection, stage: Stage) -> Result<()> {
    let references = references()?;
    let selected = |shape: Shape| -> Shape {
        shape
            .into_iter()
            .filter(|(_, object)| references.tables.contains(&object.1))
            .collect()
    };
    let expected = selected(upgraded_reference(conn, references.stages[&stage].clone())?);
    let actual = selected(legacy_shape(conn)?);
    if actual != expected {
        let changed: BTreeSet<_> = actual
            .keys()
            .chain(expected.keys())
            .filter(|key| actual.get(*key) != expected.get(*key))
            .collect();
        return Err(invalid(format!("physical {stage:?} mismatch: {changed:?}")));
    }
    Ok(())
}

fn reject_early_tail(conn: &Connection, current: i64) -> Result<()> {
    if current == 113 {
        return Ok(());
    }
    let references = references()?;
    let actual = legacy_shape(conn)?;
    let base = &references.stages[&Stage::Base];
    for (name, object) in &actual {
        if references.tables.contains(&object.1) && !base.contains_key(name) {
            return Err(invalid(format!(
                "published object before its receipt: {name}"
            )));
        }
    }
    for (table, fields) in &references.new_columns {
        if super::table_exists(conn, table)?
            && super::table_columns(conn, table)?
                .iter()
                .any(|field| fields.contains(field))
        {
            return Err(invalid(format!(
                "published column before its receipt: {table}"
            )));
        }
    }
    Ok(())
}

fn verify_103_features(conn: &Connection, current: i64, rows: &[(i64, String)]) -> Result<()> {
    let passkeys = super::table_exists(conn, "passkey_metadata")?;
    let retention = super::column_exists(conn, "backend_jobs", "input_disposition")?;
    if current < 103 {
        if passkeys || retention {
            return Err(invalid("schema-103 objects before their receipt"));
        }
        return Ok(());
    }
    if current >= 104 {
        if !passkeys || !retention {
            return Err(invalid("missing converged schema-103 objects"));
        }
    } else {
        let receipt = rows
            .iter()
            .find(|row| row.0 == 103)
            .map(|row| row.1.as_str());
        let matched = match receipt {
            Some(super::SCHEMA_BASELINE_NAME) => passkeys != retention,
            Some("Passkey metadata and atomic addition results") => passkeys && !retention,
            Some("Job input retirement independent of immutable receipts") => {
                retention && !passkeys
            }
            _ => false,
        };
        if !matched {
            return Err(invalid("schema-103 receipt-shape mismatch"));
        }
    }
    // Both migration branches have externally visible immutable receipts and
    // fences. Missing/modified triggers are not repaired opportunistically.
    let expected = &references()?.stages[&Stage::Base];
    let actual = snapshot(conn)?;
    for name in [
        "passkey_metadata",
        "passkey_addition_receipts",
        "passkey_metadata_insert",
        "backend_job_identity_immutable",
        "backend_job_input_retirement",
    ] {
        let required = if name.starts_with("passkey_") {
            passkeys
        } else {
            retention
        };
        if required && actual.get(name) != expected.get(name) {
            return Err(invalid(format!(
                "schema-103 object missing or drifted: {name}"
            )));
        }
    }
    Ok(())
}

// Only private empty reference DBs reach this function. In particular, the Job
// rebuild changes empty reference DDL; it never transforms any saved Job state.
fn install_reference(conn: &Connection, stage: Stage) -> Result<()> {
    match stage {
        Stage::Base => {}
        Stage::Published(family, count) => {
            for index in 0..count {
                reference_stage(conn, family, index)?;
            }
        }
        Stage::Converged(version) => {
            for family in FAMILIES {
                for index in 0..family.names().len() {
                    reference_stage(conn, family, index)?;
                }
            }
            for sql in [ROOT_SQL, SET_IDENTITIES_SQL, HISTORY_SQL, SYNC_SQL] {
                conn.execute_batch(sql)?;
            }
            if version >= 120 {
                conn.execute_batch(PRIOR_RELEASE_SQL)?;
            }
            if version >= 121 {
                conn.execute_batch(CLEANUP_COVERAGE_SQL)?;
            }
        }
    }
    Ok(())
}

fn reference_stage(conn: &Connection, family: Family, index: usize) -> Result<()> {
    let sql = match (family, index) {
        (Family::Permissions, 0) => PERMISSION_SQL,
        (Family::Permissions, 1) => REGISTRATIONS_SQL,
        (Family::Removal, 0) => COLLECTION_SQL,
        (Family::Removal, 1) => IDENTITY_SQL,
        (Family::Removal, 2) => PROVISIONING_SQL,
        (Family::Removal, 3) => SUPERSESSION_SQL,
        (Family::Jobs, 0) => CLAIMS_SQL,
        (Family::Jobs, 1) => CLEANUP_REMOVALS_SQL,
        (Family::Jobs, 2) => {
            reference_job_shape(conn)?;
            return Ok(());
        }
        (Family::Jobs, 3) => INVOCATION_ENDS_SQL,
        (Family::Jobs, 4) => INVOCATION_OWNERS_SQL,
        _ => return Err(invalid("invalid reference stage")),
    };
    conn.execute_batch(sql)?;
    Ok(())
}

fn reference_job_shape(conn: &Connection) -> Result<()> {
    for table in ["backend_jobs", "backend_job_attempts"] {
        let count: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })?;
        if count != 0 {
            return Err(invalid("reference rebuild requires EMPTY Job tables"));
        }
    }
    let programs = conn.prepare("SELECT type,name,sql FROM sqlite_schema WHERE type='trigger' OR (type='index' AND tbl_name IN ('backend_jobs','backend_job_attempts') AND sql IS NOT NULL) ORDER BY name")?
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (kind, name, _) in &programs {
        conn.execute_batch(&format!("DROP {kind} \"{}\";", name.replace('"', "\"\"")))?;
    }
    for (table, old, new) in [
        (
            "backend_jobs",
            "CHECK (state IN ('pending', 'completed', 'failed', 'unknown'))",
            "CHECK (state IN ('pending', 'completed', 'failed'))",
        ),
        (
            "backend_job_attempts",
            "CHECK (state IN ('reserved', 'dispatching', 'dispatched', 'completed', 'failed', 'unknown'))",
            "CHECK (state IN ('reserved', 'dispatching', 'dispatched', 'completed', 'failed'))",
        ),
    ] {
        let ddl: String = conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name=?1",
            [table],
            |row| row.get(0),
        )?;
        if ddl.matches(old).count() != 1 {
            return Err(invalid("unrecognized reference Job DDL"));
        }
        conn.execute_batch(&format!("DROP TABLE {table};"))?;
        conn.execute_batch(&ddl.replacen(old, new, 1))?;
    }
    conn.execute_batch("ALTER TABLE backend_job_attempts ADD COLUMN effect_admission_closed INTEGER NOT NULL DEFAULT 0 CHECK (effect_admission_closed IN (0, 1));")?;
    conn.execute_batch(STATE_HISTORY_SQL)?;
    for (_, _, sql) in programs {
        conn.execute_batch(&sql)?;
    }
    Ok(())
}

// Normalize only SQL syntax, never string literals. Independent column/constraint
// declarations may move under ALTER TABLE, but composite keys retain their order.
fn tokens(sql: &str) -> Result<Vec<String>> {
    let chars: Vec<_> = sql.chars().collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i..].starts_with(&['-', '-']) {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if chars[i..].starts_with(&['/', '*']) {
            i += 2;
            while i + 1 < chars.len() && !chars[i..].starts_with(&['*', '/']) {
                i += 1;
            }
            if i + 1 >= chars.len() {
                return Err(invalid("unterminated SQL comment"));
            }
            i += 2;
            continue;
        }
        if ['\'', '"', '`', '['].contains(&ch) {
            let end = if ch == '[' { ']' } else { ch };
            i += 1;
            let mut word = String::new();
            loop {
                let next = *chars
                    .get(i)
                    .ok_or_else(|| invalid("unterminated SQL quote"))?;
                i += 1;
                if next == end {
                    if end != ']' && chars.get(i) == Some(&end) {
                        word.push(end);
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    word.push(next);
                }
            }
            result.push(if ch == '\'' {
                format!("literal:{word}")
            } else {
                format!("word:{}", word.to_ascii_lowercase())
            });
        } else if ch.is_alphanumeric() || ch == '_' {
            let begin = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            result.push(format!(
                "word:{}",
                chars[begin..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase()
            ));
        } else {
            result.push(ch.to_string());
            i += 1;
        }
    }
    Ok(result)
}

fn declarations(tokens: Vec<String>) -> Result<Vec<String>> {
    let start = tokens
        .iter()
        .position(|token| token == "(")
        .ok_or_else(|| invalid("missing table declarations"))?;
    let mut depth = 1;
    let mut begin = start + 1;
    let mut fields = Vec::new();
    for i in begin..tokens.len() {
        match tokens[i].as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 || (depth == 1 && tokens[i] == ",") {
            fields.push(tokens[begin..i].to_vec());
            begin = i + 1;
        }
        if depth == 0 {
            fields.sort();
            let mut result = tokens[..=start].to_vec();
            for (index, field) in fields.into_iter().enumerate() {
                if index != 0 {
                    result.push(",".into());
                }
                result.extend(field);
            }
            result.extend_from_slice(&tokens[i..]);
            return Ok(result);
        }
    }
    Err(invalid("unbalanced table declarations"))
}

#[cfg(test)]
#[path = "storage_format_tests.rs"]
mod tests;

#[cfg(test)]
mod failure_format_tests {
    use super::super as store;
    use super::*;
    use rusqlite::params;

    fn apply(conn: &Connection, version: i64) -> Result<()> {
        let migration = store::MIGRATIONS
            .iter()
            .find(|migration| migration.version == version)
            .ok_or_else(|| invalid(format!("missing migration {version}")))?;
        (migration.apply)(conn)
    }

    fn fixture_123(stage: Stage) -> Connection {
        let conn = reference_base().unwrap();
        install_reference(&conn, stage).unwrap();
        let version = match stage {
            Stage::Base => 113,
            Stage::Published(_, count) => 113 + count as i64,
            Stage::Converged(version) => version,
        };
        conn.execute("DELETE FROM __yoi_schema_migrations", [])
            .unwrap();
        conn.execute(
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
            params![version, store::SCHEMA_BASELINE_NAME],
        )
        .unwrap();
        apply(&conn, 122).unwrap();
        apply(&conn, 123).unwrap();
        conn
    }

    // Capture every old column, including identities, results, receipts and
    // fences. Later nullable columns are tested separately, not mistaken for
    // changes to retained values. Include the ledger when checking rollback.
    fn columns(conn: &Connection) -> BTreeMap<String, Vec<String>> {
        let mut tables: BTreeSet<_> = snapshot(conn)
            .unwrap()
            .into_values()
            .filter(|object| object.0 == "table")
            .map(|object| object.1)
            .collect();
        tables.insert("__yoi_schema_migrations".into());
        tables
            .into_iter()
            .map(|table| {
                let columns = store::table_columns(conn, &table).unwrap();
                (table, columns)
            })
            .collect()
    }

    fn values(
        conn: &Connection,
        columns: &BTreeMap<String, Vec<String>>,
    ) -> BTreeMap<String, Vec<String>> {
        columns
            .iter()
            .map(|(table, columns)| {
                let columns = columns
                    .iter()
                    .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(",");
                let mut statement = conn
                    .prepare(&format!("SELECT {columns} FROM \"{table}\""))
                    .unwrap();
                let count = statement.column_count();
                let mut rows = statement
                    .query_map([], |row| {
                        (0..count)
                            .map(|index| row.get::<_, rusqlite::types::Value>(index))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .map(|row| format!("{:?}", row.unwrap()))
                    .collect::<Vec<_>>();
                rows.sort();
                (table.clone(), rows)
            })
            .collect()
    }

    #[test]
    fn failure_tail_plans_and_fresh_baselines_keep_exact_physical_shapes() {
        for stage in [
            Stage::Base,
            Stage::Published(Family::Permissions, 1),
            Stage::Published(Family::Jobs, 5),
            Stage::Converged(121),
        ] {
            let conn = fixture_123(stage);
            for current in 123..=store::LATEST_SCHEMA_VERSION {
                if current > 123 {
                    apply(&conn, current).unwrap();
                }
                let before = snapshot(&conn).unwrap();
                let history = store::workspace_schema_migration_history(&conn).unwrap();
                let plan = published_plan(&conn, current).unwrap().unwrap();
                assert_eq!(plan.current_schema_version, current);
                assert_eq!(plan.target_schema_version, store::LATEST_SCHEMA_VERSION);
                assert_eq!(
                    plan.migrations
                        .iter()
                        .map(|step| (step.version, step.name.as_str()))
                        .collect::<Vec<_>>(),
                    ((current + 1)..=store::LATEST_SCHEMA_VERSION)
                        .map(|version| (version, tail_migration_name(version).unwrap()))
                        .collect::<Vec<_>>()
                );
                assert_eq!(snapshot(&conn).unwrap(), before);
                assert_eq!(
                    store::workspace_schema_migration_history(&conn).unwrap(),
                    history
                );
            }
        }
        // Fresh composition has the Base shape, not fabricated published-family
        // receipts. Each supported additive intermediate is a real baseline.
        for current in 123..=store::LATEST_SCHEMA_VERSION {
            let conn = fixture_123(Stage::Base);
            for version in 124..=current {
                apply(&conn, version).unwrap();
            }
            conn.execute("DELETE FROM __yoi_schema_migrations", [])
                .unwrap();
            conn.execute(
                "INSERT INTO __yoi_schema_migrations(version,name) VALUES(?1,?2)",
                params![current, store::SCHEMA_BASELINE_NAME],
            )
            .unwrap();
            let saved_columns = columns(&conn);
            let saved = values(&conn, &saved_columns);
            verify_history(&conn, current).unwrap();
            assert_eq!(values(&conn, &saved_columns), saved);
        }
    }

    #[test]
    fn retained_failure_tail_adds_connection_evidence_without_relabeling_receipts() {
        let conn = fixture_123(Stage::Base);
        store::migrate_failure_extension(
            &conn,
            123,
            124,
            store::backend_job_recovery_store::CAUSE_MIGRATION_NAME,
            store::backend_job_recovery_store::CAUSE_SCHEMA,
        )
        .unwrap();
        for migration in store::RETAINED_FAILURE_MIGRATIONS
            .iter()
            .filter(|migration| migration.version <= 126)
        {
            (migration.apply)(&conn).unwrap();
        }
        let retained = store::workspace_schema_migration_history(&conn).unwrap();
        let plan = published_plan(&conn, 126).unwrap().unwrap();
        assert_eq!(
            plan.migrations
                .iter()
                .map(|step| step.version)
                .collect::<Vec<_>>(),
            vec![127, 128, 129]
        );
        apply(&conn, 127).unwrap();
        apply(&conn, 128).unwrap();
        apply(&conn, 129).unwrap();
        verify_history(&conn, 129).unwrap();
        let history = store::workspace_schema_migration_history(&conn).unwrap();
        assert_eq!(&history[..retained.len()], retained.as_slice());
        assert_eq!(
            history.last().unwrap().1,
            store::DOMAIN_FAILURE_OBSERVATIONS_MIGRATION_NAME
        );
        let fresh = Connection::open_in_memory().unwrap();
        store::prepare_connection(&fresh).unwrap();
        let select = |shape: Shape| -> Shape {
            shape
                .into_iter()
                .filter(|(_, object)| object.1 == "external_workdir_connection_observations")
                .collect()
        };
        assert_eq!(
            select(snapshot(&conn).unwrap()),
            select(snapshot(&fresh).unwrap())
        );
    }

    #[test]
    fn failure_tail_mismatches_are_rejected_without_repair() {
        for (current, sql) in [
            (127, crate::external_workdir_connection_observations::SCHEMA),
            (128, crate::external_workdir_connection_observations::SCHEMA),
            (
                129,
                "DROP TRIGGER external_workdir_connection_observations_no_update",
            ),
            (
                129,
                "UPDATE __yoi_schema_migrations SET name='counterfeit connection receipt' WHERE version=129",
            ),
            (127, crate::config_publication::SCHEMA),
            (128, crate::config_publication::SCHEMA),
            (
                128,
                "UPDATE __yoi_schema_migrations SET name='Independent configuration publication and Workdir connection observations' WHERE version=128",
            ),
            (128, "DELETE FROM ticket_schema_migrations WHERE version=17"),
            (
                129,
                "DROP TRIGGER workspace_configuration_publication_no_update",
            ),
            (
                129,
                "UPDATE __yoi_schema_migrations SET name='counterfeit publication receipt' WHERE version=129",
            ),
            (123, store::backend_job_recovery_store::CAUSE_SCHEMA),
            (124, crate::plugin_service::FAILURE_MIGRATION_SQL),
            (
                124,
                "UPDATE __yoi_schema_migrations SET name='wrong cause receipt' WHERE version=124",
            ),
            (125, "DELETE FROM __yoi_schema_migrations WHERE version=124"),
            (
                125,
                "UPDATE __yoi_schema_migrations SET name='wrong service receipt' WHERE version=125",
            ),
            (
                126,
                "ALTER TABLE plugin_service_consumers DROP COLUMN last_outcome_json",
            ),
            (
                126,
                "ALTER TABLE plugin_service_consumers DROP COLUMN last_outcome_json; ALTER TABLE plugin_service_consumers ADD COLUMN last_outcome_json BLOB",
            ),
            (
                125,
                "ALTER TABLE plugin_service_consumers ADD COLUMN unexpected TEXT",
            ),
            (
                125,
                "CREATE TRIGGER unexpected_service_trigger AFTER UPDATE ON plugin_service_consumers BEGIN SELECT 1; END",
            ),
            (
                125,
                "ALTER TABLE backend_jobs DROP COLUMN failure_cause; ALTER TABLE backend_jobs ADD COLUMN failure_cause TEXT",
            ),
            (
                125,
                "ALTER TABLE backend_job_attempts DROP COLUMN failure_cause; ALTER TABLE backend_job_attempts ADD COLUMN failure_cause TEXT CHECK(failure_cause IS NULL OR length(failure_cause)<=32768)",
            ),
            (
                125,
                "ALTER TABLE backend_job_attempts DROP COLUMN worker_cleanup_failure_cause",
            ),
            (125, "ALTER TABLE backend_jobs ADD COLUMN unexpected TEXT"),
            (
                125,
                "UPDATE __yoi_schema_migrations SET version=126 WHERE version=125",
            ),
        ] {
            let conn = fixture_123(Stage::Base);
            for version in 124..=current {
                apply(&conn, version).unwrap();
            }
            // Damage isolated fixtures only; no production downgrade/repair API.
            conn.execute_batch(sql).unwrap();
            let actual = store::current_schema_version(&conn).unwrap();
            let before = snapshot(&conn).unwrap();
            let saved_columns = columns(&conn);
            let saved = values(&conn, &saved_columns);
            assert!(verify_history(&conn, actual).is_err(), "{current}: {sql}");
            assert!(published_plan(&conn, actual).is_err(), "{current}: {sql}");
            assert_eq!(snapshot(&conn).unwrap(), before);
            assert_eq!(values(&conn, &saved_columns), saved);
        }
    }

    #[test]
    fn failure_tail_receipt_failure_rolls_back_all_ddl_and_retained_values() {
        let conn = fixture_123(Stage::Base);
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
            VALUES('owner','user','owner','Owner','1','1');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
            VALUES('workspace-a','owner','Saved workspace','active','1','1');
            INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,created_at,updated_at,input_disposition)
            VALUES('workspace-a','saved-job','journal','digest','input','{}','fingerprint','completed',1,'{\"saved\":true}','result-digest','1','1','erased');
            INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,result_json,result_digest,created_at,updated_at)
            VALUES('workspace-a','saved-job','saved-attempt',1,'digest','completed','2','{\"saved\":true}','result-digest','1','1');
            INSERT INTO plugin_service_consumers(workspace_id,consumer_id,runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json,cursor,last_ack_event_id)
            VALUES('workspace-a','saved-consumer','runtime','worker','installation','plan','watch','[\"changed\"]',7,'saved-event');").unwrap();
        conn.execute("INSERT INTO plugin_installations VALUES('workspace-a','installation','alias','package','install',0)", []).unwrap();
        let fingerprint = format!("sha256:{}", "a".repeat(64));
        conn.execute("INSERT INTO plugin_execution_plans VALUES('workspace-a','plan','owner','plan',?1,'{}','active')", [&fingerprint]).unwrap();
        assert!(
            conn.prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
        let mut old_columns = columns(&conn);
        old_columns.remove("__yoi_schema_migrations");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM typed_ticket_orchestration_plans",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        old_columns.remove("typed_ticket_orchestration_plans");
        old_columns.remove("ticket_schema_migrations");
        let component_receipts = values(
            &conn,
            &BTreeMap::from([(
                "ticket_schema_migrations".into(),
                store::table_columns(&conn, "ticket_schema_migrations").unwrap(),
            )]),
        );
        let retained = values(&conn, &old_columns);
        for version in 124..=store::LATEST_SCHEMA_VERSION {
            conn.execute_batch(&format!("CREATE TRIGGER inject_failure_receipt BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version={version} BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;")).unwrap();
            let before = snapshot(&conn).unwrap();
            let rollback_columns = columns(&conn);
            let before_rows = values(&conn, &rollback_columns);
            assert!(apply(&conn, version).is_err());
            assert_eq!(store::current_schema_version(&conn).unwrap(), version - 1);
            assert_eq!(snapshot(&conn).unwrap(), before);
            assert_eq!(values(&conn, &rollback_columns), before_rows);
            verify_history(&conn, version - 1).unwrap();
            conn.execute_batch("DROP TRIGGER inject_failure_receipt")
                .unwrap();
            apply(&conn, version).unwrap();
            verify_history(&conn, version).unwrap();
            assert_eq!(values(&conn, &old_columns), retained);
            let component = values(
                &conn,
                &BTreeMap::from([(
                    "ticket_schema_migrations".into(),
                    store::table_columns(&conn, "ticket_schema_migrations").unwrap(),
                )]),
            );
            for row in &component_receipts["ticket_schema_migrations"] {
                assert!(component["ticket_schema_migrations"].contains(row));
            }
            assert_eq!(
                component["ticket_schema_migrations"].len(),
                component_receipts["ticket_schema_migrations"].len() + usize::from(version >= 128)
            );
            assert!(
                apply(&conn, version).is_err(),
                "migration must not run twice"
            );
        }
        let nulls: (Option<String>, Option<String>, Option<String>, Option<String>) = conn.query_row(
            "SELECT j.failure_cause,a.failure_cause,a.worker_cleanup_failure_cause,s.last_outcome_json FROM backend_jobs j JOIN backend_job_attempts a USING(workspace_id,job_id) JOIN plugin_service_consumers s USING(workspace_id)",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).unwrap();
        assert_eq!(nulls, (None, None, None, None));
        // All three Job evidence columns are byte-bounded, not character-bounded;
        // cleanup evidence remains independent of the saved successful result.
        for (table, column) in [
            ("backend_jobs", "failure_cause"),
            ("backend_job_attempts", "failure_cause"),
            ("backend_job_attempts", "worker_cleanup_failure_cause"),
        ] {
            assert!(
                conn.execute(
                    &format!("UPDATE {table} SET {column}=?1"),
                    ["é".repeat(16385)]
                )
                .is_err()
            );
            conn.execute(
                &format!("UPDATE {table} SET {column}=?1"),
                ["é".repeat(16384)],
            )
            .unwrap();
        }
        assert_eq!(values(&conn, &old_columns), retained);
    }

    #[test]
    fn oldest_supported_baseline_reaches_current_through_real_runner_and_reopens() {
        assert_eq!(store::OLDEST_SCHEMA_VERSION, 50);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oldest.db");
        store::tests::prepare_schema_v50(&path, Some("workspace-a"));
        let conn = Connection::open(&path).unwrap();
        let old_columns = columns(&conn);
        let before = values(&conn, &old_columns);
        let identity_columns = BTreeMap::from([
            (
                "accounts".to_owned(),
                vec![
                    "account_id".to_owned(),
                    "kind".to_owned(),
                    "created_at".to_owned(),
                ],
            ),
            (
                "workspaces".to_owned(),
                vec![
                    "workspace_id".to_owned(),
                    "owner_account_id".to_owned(),
                    "created_at".to_owned(),
                ],
            ),
        ]);
        let identities = values(&conn, &identity_columns);
        let shape = snapshot(&conn).unwrap();
        drop(conn);
        let plan = store::SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_eq!(plan.current_schema_version, 50);
        assert_eq!(plan.target_schema_version, store::LATEST_SCHEMA_VERSION);
        assert_eq!(
            plan.migrations
                .iter()
                .rev()
                .take(5)
                .map(|step| step.version)
                .collect::<Vec<_>>(),
            vec![129, 128, 127, 126, 125]
        );
        let conn = Connection::open(&path).unwrap();
        assert_eq!(snapshot(&conn).unwrap(), shape, "dry-run changed DDL");
        assert_eq!(
            values(&conn, &old_columns),
            before,
            "dry-run changed saved values"
        );
        drop(conn);
        assert_eq!(
            store::SqliteWorkspaceStore::migrate_database(&path).unwrap(),
            plan
        );
        let conn = Connection::open(&path).unwrap();
        verify_history(&conn, store::LATEST_SCHEMA_VERSION).unwrap();
        assert_eq!(values(&conn, &identity_columns), identities);
        let history = store::workspace_schema_migration_history(&conn).unwrap();
        assert_eq!(
            history.first(),
            Some(&(50, store::SCHEMA_BASELINE_NAME.to_owned()))
        );
        assert_eq!(
            &history[history.len() - 5..],
            &[
                (125, tail_migration_name(125).unwrap().to_owned()),
                (126, tail_migration_name(126).unwrap().to_owned()),
                (127, tail_migration_name(127).unwrap().to_owned()),
                (128, tail_migration_name(128).unwrap().to_owned()),
                (129, tail_migration_name(129).unwrap().to_owned()),
            ]
        );
        let upgraded_shape = snapshot(&conn).unwrap();
        let reopened_columns = columns(&conn);
        let saved = values(&conn, &reopened_columns);
        drop(conn);
        let conn = Connection::open(&path).unwrap();
        store::prepare_connection(&conn).unwrap();
        assert_eq!(snapshot(&conn).unwrap(), upgraded_shape);
        assert_eq!(values(&conn, &reopened_columns), saved);
        // Compare migration-owned tables/programs with real fresh composition,
        // retaining baseline50's legitimate historical physical differences.
        let fresh = Connection::open_in_memory().unwrap();
        store::prepare_connection(&fresh).unwrap();
        for (name, object) in snapshot(&fresh).unwrap() {
            if [
                "backend_jobs",
                "backend_job_attempts",
                "plugin_service_consumers",
                "workspace_configuration_publication_observations",
                "external_workdir_connection_observations",
            ]
            .contains(&object.1.as_str())
            {
                assert_eq!(upgraded_shape.get(&name), Some(&object), "{name}");
            }
        }
    }

    #[test]
    fn canonical_added_columns_match_sqlite_alter_table_shape() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE sample(z TEXT, a TEXT, CHECK(length(z)>0));")
            .unwrap();
        let mut expected = snapshot(&conn).unwrap().remove("sample").unwrap().2;
        append_reference_column(&mut expected, "failure_cause TEXT CHECK(failure_cause IS NULL OR length(CAST(failure_cause AS BLOB))<=32768)").unwrap();
        conn.execute_batch("ALTER TABLE sample ADD COLUMN failure_cause TEXT CHECK(failure_cause IS NULL OR length(CAST(failure_cause AS BLOB))<=32768);").unwrap();
        assert_eq!(snapshot(&conn).unwrap()["sample"].2, expected);
    }
}

// Exact, frozen physical references from public Git 7b39a44ca698c474287e5511f674f2e8baae4c03.
// Per-constant source paths below permit byte-for-byte provenance checks. These
// are NOT registered migrations and must never be applied to a source database.

// crates/workspace-server/src/schema_convergence/sql/t818_permission_metadata.sql
const PERMISSION_SQL: &str = r####"-- Canonical permission metadata. Policy values live only in the evaluated Decodal
-- source; history/receipts are immutable evidence, never a writable policy overlay.
CREATE TABLE permission_authority_transfers (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    transfer_id TEXT NOT NULL,
    export_digest TEXT NOT NULL,
    export_json TEXT NOT NULL,
    config_identity_json TEXT NOT NULL,
    UNIQUE(workspace_id,transfer_id)
);
CREATE TABLE permission_policy_changes (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    subject_id TEXT NOT NULL,
    change_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    present INTEGER NOT NULL CHECK(present IN (0,1)),
    PRIMARY KEY(workspace_id,subject_id,change_id)
);
CREATE TABLE permission_policy_heads (
    workspace_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    change_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,subject_id),
    FOREIGN KEY(workspace_id,subject_id,change_id)
        REFERENCES permission_policy_changes(workspace_id,subject_id,change_id) ON DELETE RESTRICT
);
CREATE TABLE permission_policy_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id,operation_id)
);
CREATE TABLE permission_child_grants (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    source_change_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id,runtime_id,worker_id),
    FOREIGN KEY(workspace_id,subject_id,source_change_id)
        REFERENCES permission_policy_changes(workspace_id,subject_id,change_id) ON DELETE RESTRICT
);
-- These are permission-side fences, not Subject lifecycle state. The Host must
-- commit its canonical deletion fence first and pass the same deletion identity.
CREATE TABLE permission_legacy_workers (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    source_change_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id,runtime_id,worker_id)
);
CREATE TABLE permission_subject_fences (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE RESTRICT,
    subject_id TEXT NOT NULL,
    deletion_operation_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,subject_id)
);
CREATE TABLE permission_worker_fences (
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    deletion_operation_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,runtime_id,worker_id),
    FOREIGN KEY(workspace_id,subject_id) REFERENCES permission_subject_fences(workspace_id,subject_id) ON DELETE RESTRICT
);
CREATE TRIGGER permission_changes_immutable BEFORE UPDATE ON permission_policy_changes
BEGIN SELECT RAISE(ABORT,'permission history is immutable'); END;
CREATE TRIGGER permission_changes_retained BEFORE DELETE ON permission_policy_changes
BEGIN SELECT RAISE(ABORT,'permission history is retained'); END;
CREATE TRIGGER permission_receipts_immutable BEFORE UPDATE ON permission_policy_receipts
BEGIN SELECT RAISE(ABORT,'permission receipt is immutable'); END;
CREATE TRIGGER permission_receipts_retained BEFORE DELETE ON permission_policy_receipts
BEGIN SELECT RAISE(ABORT,'permission receipt is retained'); END;
CREATE TRIGGER permission_transfer_immutable BEFORE UPDATE ON permission_authority_transfers
BEGIN SELECT RAISE(ABORT,'permission transfer is immutable'); END;
CREATE TRIGGER permission_transfer_retained BEFORE DELETE ON permission_authority_transfers
BEGIN SELECT RAISE(ABORT,'permission transfer is retained'); END;
CREATE TRIGGER permission_grants_immutable BEFORE UPDATE ON permission_child_grants
BEGIN SELECT RAISE(ABORT,'permission child binding is immutable'); END;
CREATE TRIGGER permission_grants_retained BEFORE DELETE ON permission_child_grants
BEGIN SELECT RAISE(ABORT,'permission child binding is retained'); END;
CREATE TRIGGER permission_legacy_workers_immutable BEFORE UPDATE ON permission_legacy_workers
BEGIN SELECT RAISE(ABORT,'legacy permission binding is immutable'); END;
CREATE TRIGGER permission_legacy_workers_retained BEFORE DELETE ON permission_legacy_workers
BEGIN SELECT RAISE(ABORT,'legacy permission binding is retained'); END;
CREATE TRIGGER permission_subject_fence_immutable BEFORE UPDATE ON permission_subject_fences
BEGIN SELECT RAISE(ABORT,'permission deletion fence is immutable'); END;
CREATE TRIGGER permission_subject_fence_retained BEFORE DELETE ON permission_subject_fences
BEGIN SELECT RAISE(ABORT,'permission deletion fence is permanent'); END;
CREATE TRIGGER permission_worker_fence_immutable BEFORE UPDATE ON permission_worker_fences
BEGIN SELECT RAISE(ABORT,'permission Worker fence is immutable'); END;
CREATE TRIGGER permission_worker_fence_retained BEFORE DELETE ON permission_worker_fences
BEGIN SELECT RAISE(ABORT,'permission Worker fence is permanent'); END;
"####;

// crates/workspace-server/src/schema_convergence/sql/t818_plugin_permission_registrations.sql
const REGISTRATIONS_SQL: &str = r####"-- Applied by the ordered Workspace schema migration, never by a runtime reader.
CREATE TABLE plugin_permission_registrations (
    workspace_id TEXT NOT NULL,
    installation_id TEXT NOT NULL,
    artifact_digest TEXT NOT NULL,
    generation TEXT NOT NULL CHECK(length(generation) = 64 AND generation NOT GLOB '*[^0-9a-f]*'),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    PRIMARY KEY(workspace_id, installation_id, artifact_digest),
    FOREIGN KEY(workspace_id, installation_id, artifact_digest)
        REFERENCES plugin_artifacts(workspace_id, installation_id, artifact_digest) ON DELETE CASCADE
);

-- Old installations start a new lineage rather than inheriting historical grants.
INSERT INTO plugin_permission_registrations
    (workspace_id, installation_id, artifact_digest, generation, enabled)
SELECT workspace_id, installation_id, artifact_digest, lower(hex(randomblob(32))), 1
FROM plugin_references WHERE kind = 'config' AND owner = 'workspace-config';

-- Config synchronization must preserve unchanged references: delete/reinsert is
-- a new activation and intentionally invalidates that reference's frozen grants.
CREATE TRIGGER plugin_permission_enable AFTER INSERT ON plugin_references
WHEN NEW.kind = 'config' AND NEW.owner = 'workspace-config'
BEGIN
    INSERT INTO plugin_permission_registrations
        (workspace_id, installation_id, artifact_digest, generation, enabled)
    VALUES(NEW.workspace_id, NEW.installation_id, NEW.artifact_digest, lower(hex(randomblob(32))), 1)
    ON CONFLICT(workspace_id, installation_id, artifact_digest)
    DO UPDATE SET generation = excluded.generation, enabled = 1;
END;

CREATE TRIGGER plugin_permission_disable AFTER DELETE ON plugin_references
WHEN OLD.kind = 'config' AND OLD.owner = 'workspace-config'
BEGIN
    UPDATE plugin_permission_registrations SET enabled = 0
    WHERE workspace_id = OLD.workspace_id AND installation_id = OLD.installation_id
        AND artifact_digest = OLD.artifact_digest;
END;
"####;

// crates/workspace-server/src/schema_convergence/sql/t821_removal_collection.sql
const COLLECTION_SQL: &str =
    r####"ALTER TABLE worker_removal_operations ADD COLUMN journal_collection_json TEXT;"####;

// crates/workspace-server/src/schema_convergence/sql/t821_removal_identity.sql
const IDENTITY_SQL: &str =
    r####"ALTER TABLE worker_removal_operations ADD COLUMN journal_storage_identity_json TEXT;"####;

// crates/workspace-server/src/schema_convergence/sql/t821_workspace_feature_provisioning.sql
const PROVISIONING_SQL: &str = r####"
CREATE TABLE workspace_feature_provisioning (
    creation_operation_key TEXT NOT NULL,
    identity_operation_key TEXT NOT NULL,
    request_fingerprint TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    feature_id TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved', 'issued', 'confirmed')),
    PRIMARY KEY (creation_operation_key, feature_id),
    UNIQUE (workspace_id, feature_id),
    FOREIGN KEY (identity_operation_key)
        REFERENCES workspace_signing_identity_provisioning_operations(operation_key)
);
"####;

// crates/workspace-server/src/schema_convergence/sql/t821_erasure_supersession.sql
const SUPERSESSION_SQL: &str = r####"-- Immutable authority/reference facts only; execution state stays on existing parents.
CREATE UNIQUE INDEX worker_removal_erasure_parent_identity
    ON worker_removal_operations(operation_id,input_fingerprint,workspace_id,runtime_id,worker_id);
CREATE TABLE worker_removal_erasure_supersessions (
    previous_operation_id TEXT PRIMARY KEY,
    replacement_operation_id TEXT NOT NULL UNIQUE,
    previous_fingerprint TEXT NOT NULL,
    replacement_fingerprint TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    session_id TEXT NOT NULL CHECK(length(session_id)>0),
    previous_request_json TEXT NOT NULL CHECK(json_valid(previous_request_json)),
    replacement_request_json TEXT NOT NULL CHECK(json_valid(replacement_request_json)),
    created_at TEXT NOT NULL,
    CHECK(previous_operation_id<>replacement_operation_id),
    CHECK(previous_fingerprint<>replacement_fingerprint),
    FOREIGN KEY(previous_operation_id,previous_fingerprint,workspace_id,runtime_id,worker_id)
        REFERENCES worker_removal_operations(operation_id,input_fingerprint,workspace_id,runtime_id,worker_id),
    FOREIGN KEY(replacement_operation_id,replacement_fingerprint,workspace_id,runtime_id,worker_id)
        REFERENCES worker_removal_operations(operation_id,input_fingerprint,workspace_id,runtime_id,worker_id)
);
CREATE TRIGGER worker_removal_erasure_supersessions_no_update
BEFORE UPDATE ON worker_removal_erasure_supersessions
BEGIN SELECT RAISE(ABORT,'Worker removal erasure supersession is immutable'); END;
CREATE TRIGGER worker_removal_erasure_supersessions_no_delete
BEFORE DELETE ON worker_removal_erasure_supersessions
BEGIN SELECT RAISE(ABORT,'Worker removal erasure supersession is immutable'); END;
"####;

// crates/workspace-server/src/schema_convergence/sql/t820_backend_job_cleanup_claims.sql
const CLAIMS_SQL: &str = r####"ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_claim_id TEXT
            CHECK (worker_cleanup_claim_id IS NULL OR length(worker_cleanup_claim_id) = 36);
         CREATE TRIGGER backend_job_cleanup_claim_insert
         BEFORE INSERT ON backend_job_attempts
         WHEN NEW.worker_cleanup_state = 'executing' AND NEW.worker_cleanup_claim_id IS NULL
         BEGIN SELECT RAISE(ABORT, 'executing cleanup requires an invocation claim'); END;
         CREATE TRIGGER backend_job_cleanup_claim_update
         BEFORE UPDATE OF worker_cleanup_state, worker_cleanup_claim_id ON backend_job_attempts
         WHEN NEW.worker_cleanup_state = 'executing' AND NEW.worker_cleanup_claim_id IS NULL
           AND NOT (OLD.worker_cleanup_state IS 'executing' AND OLD.worker_cleanup_claim_id IS NULL)
         BEGIN SELECT RAISE(ABORT, 'executing cleanup requires an invocation claim'); END;"####;

// crates/workspace-server/src/schema_convergence/sql/t820_backend_journal_cleanup_removals.sql
const CLEANUP_REMOVALS_SQL: &str = r####"CREATE TABLE backend_journal_cleanup_removals (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    claim_id TEXT NOT NULL CHECK (length(claim_id) = 36),
    input_digest TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    input_fingerprint TEXT NOT NULL,
    generation_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, job_id, attempt_id, claim_id),
    UNIQUE (operation_id),
    FOREIGN KEY (workspace_id, job_id, attempt_id)
        REFERENCES backend_job_attempts(workspace_id, job_id, attempt_id) ON DELETE CASCADE,
    FOREIGN KEY (operation_id) REFERENCES worker_removal_operations(operation_id)
);
"####;

// crates/workspace-server/src/schema_convergence/sql/t820_backend_job_state_history.sql
const STATE_HISTORY_SQL: &str = r####"
CREATE TABLE backend_job_legacy_state_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state = 'unknown'),
    completed_at TEXT,
    failure_category TEXT,
    failure_detail TEXT,
    PRIMARY KEY (workspace_id, job_id),
    FOREIGN KEY (workspace_id, job_id)
        REFERENCES backend_jobs(workspace_id, job_id) ON DELETE CASCADE
);
CREATE TABLE backend_job_attempt_legacy_state_history (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state = 'unknown'),
    completed_at TEXT,
    failure_category TEXT,
    failure_detail TEXT,
    PRIMARY KEY (workspace_id, job_id, attempt_id),
    FOREIGN KEY (workspace_id, job_id, attempt_id)
        REFERENCES backend_job_attempts(workspace_id, job_id, attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_effect_admission_monotone
BEFORE UPDATE OF effect_admission_closed ON backend_job_attempts
WHEN NEW.effect_admission_closed < OLD.effect_admission_closed
BEGIN SELECT RAISE(ABORT, 'Backend Job effect admission cannot reopen'); END;
CREATE TRIGGER backend_job_legacy_state_no_update
BEFORE UPDATE ON backend_job_legacy_state_history
BEGIN SELECT RAISE(ABORT, 'Backend Job migration history is immutable'); END;
CREATE TRIGGER backend_job_attempt_legacy_state_no_update
BEFORE UPDATE ON backend_job_attempt_legacy_state_history
BEGIN SELECT RAISE(ABORT, 'Backend Job migration history is immutable'); END;
"####;

// crates/workspace-server/src/schema_convergence/sql/t820_backend_job_cleanup_invocation_ends.sql
const INVOCATION_ENDS_SQL: &str = r####"CREATE TABLE backend_job_cleanup_invocation_ends (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    claim_id TEXT NOT NULL CHECK(length(claim_id)=36),
    ended_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id,claim_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id)
        REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_cleanup_invocation_end_immutable
BEFORE UPDATE ON backend_job_cleanup_invocation_ends
BEGIN SELECT RAISE(ABORT,'cleanup invocation termination evidence is immutable'); END;
"####;

// crates/workspace-server/src/schema_convergence/sql/t820_backend_job_cleanup_invocation_owners.sql
const INVOCATION_OWNERS_SQL: &str = r####"CREATE TABLE backend_job_cleanup_invocation_owners (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    claim_id TEXT NOT NULL,
    lease_scope TEXT NOT NULL,
    lease_device TEXT NOT NULL,
    lease_inode TEXT NOT NULL,
    acquisition_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id,claim_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id)
        REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_cleanup_owner_immutable
BEFORE UPDATE ON backend_job_cleanup_invocation_owners
BEGIN SELECT RAISE(ABORT,'cleanup invocation ownership is immutable'); END;
"####;

// crates/workspace-server/src/feature_storage_owner/root_schema.sql
const ROOT_SQL: &str = r####"-- Predecessor: composed ServerDB prefix including T821 workspace_feature_provisioning,
-- workspace_signing_identity_provisioning_operations and workspace_create_operations.
-- T816 owns ordered/prefix-aware registration; do not execute this from an opener,
-- rewrite any published 114..118 SQL, or assign a competing migration number.
CREATE TABLE server_feature_root (
    root_slot INTEGER NOT NULL PRIMARY KEY CHECK (root_slot = 1),
    reservation_id TEXT NOT NULL UNIQUE,
    request_fingerprint TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('reserved', 'issued', 'accepted')),
    accepted_receipt TEXT,
    CHECK ((state = 'accepted' AND accepted_receipt IS NOT NULL)
        OR (state != 'accepted' AND accepted_receipt IS NULL))
);
CREATE TRIGGER server_feature_root_identity_immutable
BEFORE UPDATE ON server_feature_root
WHEN NEW.root_slot != OLD.root_slot
    OR NEW.reservation_id != OLD.reservation_id
    OR NEW.request_fingerprint != OLD.request_fingerprint
    OR NOT ((OLD.state = 'reserved' AND NEW.state = 'issued')
        OR (OLD.state = 'issued' AND NEW.state = 'accepted')
        OR (OLD.state = NEW.state AND NEW.accepted_receipt IS OLD.accepted_receipt))
BEGIN SELECT RAISE(ABORT, 'Feature root identity or transition changed'); END;
CREATE TRIGGER server_feature_root_no_delete BEFORE DELETE ON server_feature_root
BEGIN SELECT RAISE(ABORT, 'Feature root acceptance cannot be deleted implicitly'); END;
"####;

// crates/workspace-server/src/plugin_permissions/set_identities_schema.sql
const SET_IDENTITIES_SQL: &str = r####"-- Separate module DDL, composed only by a new ordered Workspace migration.
-- Requires the historical Plugin permission registration schema and workspaces.
-- Extension-point identity is Workspace-owned, not owned by a live contributor.
-- No operation, namespace, registration or grant survives through this table.
CREATE TABLE workspace_permission_set_identities (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    set_id TEXT NOT NULL CHECK(length(set_id) BETWEEN 1 AND 128),
    PRIMARY KEY(workspace_id, set_id)
);
"####;

// crates/workspace-server/src/config_source/sync_schema.sql
const SYNC_SQL: &str = r####"CREATE TABLE workspace_config_sync_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id) ON DELETE CASCADE,
    operation_id TEXT NOT NULL CHECK(length(operation_id) BETWEEN 1 AND 128),
    account_id TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation_id)
) STRICT;
"####;

// crates/workspace-server/src/retention/prior_release_schema.sql
const PRIOR_RELEASE_SQL: &str = r####"-- Validation progress only: removal must reread the original release, its receipt
-- and current attempt proof. This checkpoint never grants release authority.
CREATE TABLE worker_removal_prior_releases (
    operation_id TEXT PRIMARY KEY REFERENCES worker_removal_operations(operation_id) ON DELETE CASCADE,
    original_release_operation_id TEXT NOT NULL,
    lease_json TEXT NOT NULL,
    request_json TEXT NOT NULL,
    storage_identity_json TEXT NOT NULL,
    admission_sha256 TEXT NOT NULL,
    after_session_id TEXT,
    validation_complete INTEGER NOT NULL DEFAULT 0 CHECK (validation_complete IN (0, 1))
);

CREATE TRIGGER worker_removal_prior_releases_no_update
BEFORE UPDATE ON worker_removal_prior_releases
WHEN OLD.operation_id IS NOT NEW.operation_id
  OR OLD.original_release_operation_id IS NOT NEW.original_release_operation_id
  OR OLD.lease_json IS NOT NEW.lease_json
  OR OLD.request_json IS NOT NEW.request_json
  OR OLD.storage_identity_json IS NOT NEW.storage_identity_json
  OR OLD.admission_sha256 IS NOT NEW.admission_sha256
  OR NEW.validation_complete < OLD.validation_complete
BEGIN
    SELECT RAISE(ABORT, 'prior release validation reference is immutable');
END;

CREATE TRIGGER worker_removal_prior_releases_no_delete
BEFORE DELETE ON worker_removal_prior_releases
BEGIN
    SELECT RAISE(ABORT, 'prior release validation reference is retained');
END;
"####;

// crates/workspace-server/src/backend_job_cleanup_coverage.sql
const CLEANUP_COVERAGE_SQL: &str = r####"CREATE TABLE backend_job_cleanup_history_origins (
    workspace_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    input_digest TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id,attempt_id),
    FOREIGN KEY(workspace_id,job_id,attempt_id)
        REFERENCES backend_job_attempts(workspace_id,job_id,attempt_id) ON DELETE CASCADE
);
CREATE TRIGGER backend_job_cleanup_origin_immutable
BEFORE UPDATE ON backend_job_cleanup_history_origins
BEGIN SELECT RAISE(ABORT,'cleanup history origin is immutable'); END;
CREATE TRIGGER backend_job_cleanup_origin_requires_unallocated_birth
BEFORE INSERT ON backend_job_cleanup_history_origins
WHEN NOT EXISTS(SELECT 1 FROM backend_job_attempts a
    WHERE a.workspace_id=NEW.workspace_id AND a.job_id=NEW.job_id AND a.attempt_id=NEW.attempt_id
      AND a.attempt=NEW.attempt AND a.input_digest=NEW.input_digest AND a.state='reserved'
      AND a.worker_id IS NULL AND a.runtime_id IS NULL AND a.runtime_run_id IS NULL
      AND a.worker_cleanup_state IS NULL AND a.worker_cleanup_claim_id IS NULL)
 OR EXISTS(SELECT 1 FROM worker_create_reservations r
    WHERE r.workspace_id=NEW.workspace_id AND r.allocation_key='backend-job:'||NEW.attempt_id)
 OR EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_owners o
    WHERE o.workspace_id=NEW.workspace_id AND o.job_id=NEW.job_id AND o.attempt_id=NEW.attempt_id)
 OR EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_ends e
    WHERE e.workspace_id=NEW.workspace_id AND e.job_id=NEW.job_id AND e.attempt_id=NEW.attempt_id)
BEGIN SELECT RAISE(ABORT,'cleanup history origin requires a newly inserted unallocated attempt'); END;
CREATE TRIGGER backend_job_cleanup_claim_requires_history
BEFORE UPDATE OF worker_cleanup_state,worker_cleanup_claim_id ON backend_job_attempts
WHEN NEW.worker_cleanup_state='executing'
 AND (OLD.worker_cleanup_state IS NOT 'executing' OR NEW.worker_cleanup_claim_id IS NOT OLD.worker_cleanup_claim_id)
 AND (
    NOT EXISTS(SELECT 1 FROM backend_job_cleanup_history_origins c
        WHERE c.workspace_id=NEW.workspace_id AND c.job_id=NEW.job_id AND c.attempt_id=NEW.attempt_id
          AND c.attempt=NEW.attempt AND c.input_digest=NEW.input_digest)
    OR (OLD.worker_cleanup_state IS NOT 'executing' AND NEW.worker_cleanup_claim_id IS OLD.worker_cleanup_claim_id)
    OR (OLD.worker_cleanup_state IS 'executing' AND NEW.worker_cleanup_claim_id IS NOT OLD.worker_cleanup_claim_id)
    OR (NEW.worker_cleanup_claim_id IS NOT OLD.worker_cleanup_claim_id
        AND OLD.worker_cleanup_claim_id IS NOT NULL
        AND NOT EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_ends e
            WHERE e.workspace_id=OLD.workspace_id AND e.job_id=OLD.job_id AND e.attempt_id=OLD.attempt_id
              AND e.claim_id=OLD.worker_cleanup_claim_id))
    OR EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_owners o
        WHERE o.workspace_id=OLD.workspace_id AND o.job_id=OLD.job_id AND o.attempt_id=OLD.attempt_id
          AND NOT EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_ends old_end
            WHERE old_end.workspace_id=o.workspace_id AND old_end.job_id=o.job_id
              AND old_end.attempt_id=o.attempt_id AND old_end.claim_id=o.claim_id)))
BEGIN SELECT RAISE(ABORT,'cleanup claim requires complete original invocation history'); END;
CREATE TRIGGER backend_job_cleanup_no_pending_reset
BEFORE UPDATE OF worker_cleanup_state ON backend_job_attempts
WHEN OLD.worker_cleanup_state='executing' AND NEW.worker_cleanup_state='pending'
BEGIN SELECT RAISE(ABORT,'executing cleanup cannot be reset to pending'); END;
CREATE VIEW backend_job_cleanup_ended_attempts AS
SELECT a.workspace_id,a.job_id,a.attempt_id,a.worker_cleanup_claim_id AS claim_id
FROM backend_job_attempts a
JOIN backend_job_cleanup_history_origins c
  ON c.workspace_id=a.workspace_id AND c.job_id=a.job_id AND c.attempt_id=a.attempt_id
 AND c.attempt=a.attempt AND c.input_digest=a.input_digest
JOIN backend_job_cleanup_invocation_ends e
  ON e.workspace_id=a.workspace_id AND e.job_id=a.job_id AND e.attempt_id=a.attempt_id
 AND e.claim_id=a.worker_cleanup_claim_id
WHERE NOT EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_owners o
    WHERE o.workspace_id=a.workspace_id AND o.job_id=a.job_id AND o.attempt_id=a.attempt_id
      AND NOT EXISTS(SELECT 1 FROM backend_job_cleanup_invocation_ends old_end
        WHERE old_end.workspace_id=o.workspace_id AND old_end.job_id=o.job_id
          AND old_end.attempt_id=o.attempt_id AND old_end.claim_id=o.claim_id));
"####;

// crates/content-history-sqlite/src/schema.sql
const HISTORY_SQL: &str = r####"-- Installed by the owning database's versioned migration, never on ordinary reads.
CREATE TABLE content_history_targets (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    head BLOB CHECK(head IS NULL OR length(head) = 32),
    erased INTEGER NOT NULL DEFAULT 0 CHECK(erased IN (0, 1)),
    PRIMARY KEY(workspace, target),
    CHECK(erased = 0 OR head IS NULL)
);
CREATE TABLE content_history_objects (
    workspace TEXT NOT NULL,
    kind INTEGER NOT NULL CHECK(kind IN (1, 2)),
    hash BLOB NOT NULL CHECK(length(hash) = 32),
    encoding BLOB NOT NULL,
    PRIMARY KEY(workspace, kind, hash)
);
CREATE TABLE content_history_records (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    hash BLOB NOT NULL CHECK(length(hash) = 32),
    encoding BLOB NOT NULL,
    PRIMARY KEY(workspace, target, hash),
    FOREIGN KEY(workspace, target) REFERENCES content_history_targets(workspace, target)
);
-- All retained histories own their complete closure, not just the current head.
CREATE TABLE content_history_references (
    workspace TEXT NOT NULL,
    target TEXT NOT NULL,
    history BLOB NOT NULL,
    kind INTEGER NOT NULL,
    hash BLOB NOT NULL,
    PRIMARY KEY(workspace, target, history, kind, hash),
    FOREIGN KEY(workspace, target, history)
        REFERENCES content_history_records(workspace, target, hash),
    FOREIGN KEY(workspace, kind, hash)
        REFERENCES content_history_objects(workspace, kind, hash)
);
CREATE INDEX content_history_object_owners ON content_history_references(workspace, kind, hash);
"####;
