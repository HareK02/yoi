use super::*;
use crate::{
    plugin_catalog::{self, PluginReference, PluginReferenceKind},
    store::SqliteWorkspaceStore,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPinState {
    Pending,
    Active,
    Restorable,
    Released,
}
impl PlanPinState {
    fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Restorable => "restorable",
            Self::Released => "released",
        }
    }
    fn reference_kind(self) -> Option<PluginReferenceKind> {
        match self {
            Self::Pending => Some(PluginReferenceKind::Pending),
            Self::Active => Some(PluginReferenceKind::Active),
            Self::Restorable => Some(PluginReferenceKind::Restorable),
            Self::Released => None,
        }
    }
    fn can_transition(self, target: Self) -> bool {
        self == target
            || matches!(
                (self, target),
                (Self::Pending, Self::Active | Self::Released)
                    | (Self::Active, Self::Restorable | Self::Released)
                    | (Self::Restorable, Self::Active | Self::Released)
            )
    }
}

/// Immutable creation result. `initial_pin_state` describes creation, never the
/// current retention state. Replay after release must not resurrect any pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPlanCreateReceipt {
    pub plan_id: String,
    pub fingerprint: String,
    pub initial_pin_state: PlanPinState,
    pub plan: WorkerPluginExecutionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPluginPlan {
    pub plan_id: String,
    pub fingerprint: String,
    pub pin_state: PlanPinState,
    pub plan: WorkerPluginExecutionPlan,
}

// A local savepoint keeps owner functions atomic even if a caller handles their
// error without aborting its surrounding Worker-create transaction.
fn atomic<T>(tx: &Transaction<'_>, work: impl FnOnce() -> Result<T>) -> Result<T> {
    tx.execute_batch("SAVEPOINT plugin_plan_owner")?;
    match work() {
        Ok(value) => {
            tx.execute_batch("RELEASE plugin_plan_owner")?;
            Ok(value)
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO plugin_plan_owner; RELEASE plugin_plan_owner")?;
            Err(error)
        }
    }
}

/// The caller owns the authenticated create key and an immediate transaction in
/// the *same database* as catalog and Worker creation. Include the returned plan
/// fingerprint in the Worker launch/create fingerprint. New creation rechecks
/// current config/Profile/binding provenance and admitted metadata in this same
/// transaction. Owner-supplied narrowed snapshots cannot exceed that authority.
///
/// Replay precedes catalog admission: an immutable historical result is returned
/// even after revocation, uninstall or release. It is not fresh execution authority.
pub fn plugin_plan_create(
    tx: &Transaction<'_>,
    actor: &str,
    operation_key: &str,
    plan: &WorkerPluginExecutionPlan,
) -> Result<PluginPlanCreateReceipt> {
    if !valid_identity(actor) || !valid_identity(operation_key) || operation_key.len() > 128 {
        return invalid();
    }
    plan.validate_canonical()?;
    let fingerprint = plan.fingerprint()?;
    atomic(tx, || {
        let saved = tx.query_row(
            "SELECT plan_id,fingerprint FROM plugin_execution_plans WHERE workspace_id=?1 AND actor=?2 AND operation_key=?3",
            params![plan.workspace_id(), actor, operation_key],
            |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?)),
        ).optional()?;
        if let Some((plan_id, saved_fingerprint)) = saved {
            if saved_fingerprint != fingerprint {
                return conflict();
            }
            return receipt(plugin_plan_load(tx, plan.workspace_id(), &plan_id)?);
        }
        for pin in plan.plugins() {
            plugin_catalog::plugin_artifact_admit(
                tx,
                plan.workspace_id(),
                pin.installation_id(),
                pin.expected_last_operation(),
                pin.package_digest(),
                pin.artifact_digest(),
            )?;
        }
        crate::plugin_settings::validate_plan_source(tx, plan)?;
        let plan_id = format!("pep-{}", uuid::Uuid::now_v7());
        tx.execute(
            "INSERT INTO plugin_execution_plans(workspace_id,plan_id,actor,operation_key,fingerprint,plan_json,pin_state) VALUES(?1,?2,?3,?4,?5,?6,'pending')",
            params![plan.workspace_id(), plan_id, actor, operation_key, fingerprint, plan.canonical_json()?],
        )?;
        for pin in plan.plugins() {
            let artifact = plugin_catalog::plugin_artifact_admit(
                tx,
                plan.workspace_id(),
                pin.installation_id(),
                pin.expected_last_operation(),
                pin.package_digest(),
                pin.artifact_digest(),
            )?;
            if !pin.matches_artifact(&artifact)? {
                return conflict();
            }
            plugin_catalog::plugin_reference_put(
                tx,
                plan.workspace_id(),
                &PluginReference {
                    kind: PluginReferenceKind::Pending,
                    owner: plan_id.clone(),
                    installation_id: pin.installation_id().to_owned(),
                    expected_last_operation: pin.expected_last_operation().to_owned(),
                    package_digest: pin.package_digest().to_owned(),
                    artifact_digest: pin.artifact_digest().to_owned(),
                },
            )?;
        }
        receipt(plugin_plan_load(tx, plan.workspace_id(), &plan_id)?)
    })
}
fn receipt(saved: StoredPluginPlan) -> Result<PluginPlanCreateReceipt> {
    Ok(PluginPlanCreateReceipt {
        plan_id: saved.plan_id,
        fingerprint: saved.fingerprint,
        initial_pin_state: PlanPinState::Pending,
        plan: saved.plan,
    })
}

/// Reads only persisted immutable input, never current config or a latest artifact.
/// `Released` snapshots remain readable for audit/replay but cannot be restored.
pub fn plugin_plan_load(
    conn: &Connection,
    workspace: &str,
    plan_id: &str,
) -> Result<StoredPluginPlan> {
    let row = conn.query_row(
        "SELECT fingerprint,plan_json,pin_state FROM plugin_execution_plans WHERE workspace_id=?1 AND plan_id=?2",
        params![workspace, plan_id],
        |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?, row.get::<_,String>(2)?)),
    ).optional()?;
    let Some((fingerprint, json, state)) = row else {
        return not_found();
    };
    let plan: WorkerPluginExecutionPlan = serde_json::from_str(&json)
        .map_err(|_| Error::Store("invalid persisted Plugin execution plan".into()))?;
    plan.validate_canonical()?;
    if plan.workspace_id() != workspace || plan.fingerprint()? != fingerprint {
        return Err(Error::Store(
            "Plugin execution plan integrity mismatch".into(),
        ));
    }
    let pin_state = match state.as_str() {
        "pending" => PlanPinState::Pending,
        "active" => PlanPinState::Active,
        "restorable" => PlanPinState::Restorable,
        "released" => PlanPinState::Released,
        _ => {
            return Err(Error::Store(
                "invalid Plugin execution plan pin state".into(),
            ));
        }
    };
    Ok(StoredPluginPlan {
        plan_id: plan_id.into(),
        fingerprint,
        pin_state,
        plan,
    })
}

/// Moves pins atomically with the caller's Worker lifecycle update. State is CAS
/// compared; a stale transition is not silently replayed. Releasing is always
/// possible after revoke and never re-admits an artifact. Releasing twice with
/// expected=Released is a harmless no-op. No transition out of Released is allowed.
///
/// Acquiring a different retained kind rechecks live catalog admission. It uses
/// the installation's current operation (read inside the write transaction), not
/// the creation-time observation: unrelated artifact additions do not invalidate
/// immutable snapshots. Revoked targets cannot acquire new pins; an unsuccessful
/// transition retains all old refs. A revoked Worker may release its pins, but
/// cannot become restorable/active. T-793 must still enforce live operation checks.
pub fn plugin_plan_transition(
    tx: &Transaction<'_>,
    workspace: &str,
    plan_id: &str,
    expected: PlanPinState,
    target: PlanPinState,
) -> Result<StoredPluginPlan> {
    atomic(tx, || {
        let mut saved = plugin_plan_load(tx, workspace, plan_id)?;
        if saved.pin_state != expected || !expected.can_transition(target) {
            return conflict();
        }
        if expected == target {
            return Ok(saved);
        }
        if let Some(kind) = target.reference_kind() {
            for pin in saved.plan.plugins() {
                let operation: String = tx.query_row(
                    "SELECT last_operation FROM plugin_installations WHERE workspace_id=?1 AND installation_id=?2",
                    params![workspace, pin.installation_id()], |row| row.get(0),
                )?;
                plugin_catalog::plugin_reference_put(
                    tx,
                    workspace,
                    &PluginReference {
                        kind,
                        owner: plan_id.into(),
                        installation_id: pin.installation_id().to_owned(),
                        expected_last_operation: operation,
                        package_digest: pin.package_digest().to_owned(),
                        artifact_digest: pin.artifact_digest().to_owned(),
                    },
                )?;
            }
        }
        if let Some(kind) = expected.reference_kind() {
            for pin in saved.plan.plugins() {
                plugin_catalog::plugin_reference_remove(
                    tx,
                    workspace,
                    kind,
                    plan_id,
                    pin.installation_id(),
                    pin.artifact_digest(),
                )?;
            }
        }
        let changed = tx.execute(
            "UPDATE plugin_execution_plans SET pin_state=?1 WHERE workspace_id=?2 AND plan_id=?3 AND pin_state=?4",
            params![target.name(),workspace,plan_id,expected.name()],
        )?;
        if changed != 1 {
            return conflict();
        }
        saved.pin_state = target;
        Ok(saved)
    })
}

impl SqliteWorkspaceStore {
    pub fn plugin_plan_create(
        &self,
        actor: &str,
        operation_key: &str,
        plan: &WorkerPluginExecutionPlan,
    ) -> Result<PluginPlanCreateReceipt> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let receipt = plugin_plan_create(&tx, actor, operation_key, plan)?;
            tx.commit()?;
            Ok(receipt)
        })
    }
    pub fn plugin_plan_load(&self, workspace: &str, plan_id: &str) -> Result<StoredPluginPlan> {
        self.with_conn(|conn| plugin_plan_load(conn, workspace, plan_id))
    }
    pub fn plugin_plan_transition(
        &self,
        workspace: &str,
        plan_id: &str,
        expected: PlanPinState,
        target: PlanPinState,
    ) -> Result<StoredPluginPlan> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let saved = plugin_plan_transition(&tx, workspace, plan_id, expected, target)?;
            tx.commit()?;
            Ok(saved)
        })
    }
}

pub(crate) fn verify_plan_schema(conn: &Connection) -> Result<()> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(SCHEMA_SQL)?;
    let mut statement = expected.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND tbl_name='plugin_execution_plans' ORDER BY name")?;
    for item in statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (name, sql) = item?;
        let actual = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [&name],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if actual.as_deref() != Some(&sql) {
            return Err(Error::Store(
                "Plugin plan storage schema has drifted".into(),
            ));
        }
    }
    Ok(())
}
